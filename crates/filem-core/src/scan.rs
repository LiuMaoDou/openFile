use crate::{db, model, platform::*, rules::Excludes, Control, Engine};
use anyhow::{bail, Result};
use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension, Params, Row};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
};

fn execute_cached(c: &Connection, sql: &str, params: impl Params) -> rusqlite::Result<usize> {
    c.prepare_cached(sql)?.execute(params)
}
fn query_cached<T>(
    c: &Connection,
    sql: &str,
    params: impl Params,
    read: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    c.prepare_cached(sql)?.query_row(params, read)
}

struct Found {
    path: PathBuf,
    meta: fs::Metadata,
    identity: String,
}
struct Batch {
    files: Vec<Found>,
    dirs: Vec<PathBuf>,
    failed: Vec<PathBuf>,
}
fn read_dir(
    dir: &Path,
    root: &Path,
    rules: &Excludes,
    recursive: bool,
    storage: &Path,
    gate: &crate::scan_gate::ScanGate,
) -> Batch {
    let mut batch = Batch {
        files: Vec::new(),
        dirs: Vec::new(),
        failed: Vec::new(),
    };
    let safe = fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && !reparse(attributes(&m)))
        && dir
            .canonicalize()
            .is_ok_and(|p| p.starts_with(root) && !p.starts_with(storage));
    if !safe {
        batch.failed.push(dir.into());
        return batch;
    }
    let iter = match fs::read_dir(dir) {
        Ok(i) => i,
        Err(_) => {
            batch.failed.push(dir.into());
            return batch;
        }
    };
    for item in iter {
        if gate.is_paused() {
            break;
        }
        let item = match item {
            Ok(i) => i,
            Err(_) => {
                batch.failed.push(dir.into());
                continue;
            }
        };
        let path = item.path();
        if path.starts_with(storage) || rules.matches(path.strip_prefix(root).unwrap_or(&path)) {
            continue;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                batch.failed.push(path);
                continue;
            }
        };
        let attrs = attributes(&meta);
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if recursive {
                if reparse(attrs) || placeholder(attrs) {
                    batch.failed.push(path);
                } else {
                    batch.dirs.push(path);
                }
            }
        } else if meta.is_file() {
            match identity(&path, &meta) {
                Ok(identity) => batch.files.push(Found {
                    path,
                    meta,
                    identity,
                }),
                Err(_) => batch.failed.push(path),
            }
        }
    }
    batch
}
pub(crate) fn run(engine: &Engine, id: &str, control: &Arc<Control>) -> Result<()> {
    let (root, root_identity, version, input) = {
        let c = engine.lock()?;
        db::root(&c, id)?
    };
    let meta = match fs::metadata(&root) {
        Ok(m) => m,
        Err(e) => {
            let c = engine.lock()?;
            db::set_state(
                &c,
                id,
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "access_denied"
                } else {
                    "offline"
                },
                "partial",
                Some(&e.to_string()),
            )?;
            return Ok(());
        }
    };
    if identity(&root, &meta)? != root_identity {
        let c = engine.lock()?;
        db::set_state(
            &c,
            id,
            "identity_changed",
            "partial",
            Some("根目录身份发生变化，请移除后重新选择文件夹。"),
        )?;
        return Ok(());
    }
    let _ = engine.everything_prefill(id);
    let rules = Excludes::new(&input.excludes)?;
    let generation = uuid::Uuid::new_v4().to_string();
    {
        let c = engine.lock()?;
        db::set_state(&c, id, "available", "scanning", None)?;
        c.execute("UPDATE scopes SET scanned=0 WHERE id=?", [id])?;
    }
    let mut queue = VecDeque::from([root.clone()]);
    let mut failed = Vec::new();
    let mut count = 0usize;
    'scan: while !queue.is_empty() {
        let _permit = engine.inner.scan_gate.enter();
        if control.cancel.load(Ordering::SeqCst) || control.removed.load(Ordering::SeqCst) {
            bail!("扫描已取消，保留已发现文件与上次索引。");
        }
        let dirs: Vec<_> = (0..queue.len().min(16))
            .filter_map(|_| queue.pop_front())
            .collect();
        let batches = engine.inner.pool.install(|| {
            dirs.par_iter()
                .map(|d| {
                    read_dir(
                        d,
                        &root,
                        &rules,
                        input.recursive,
                        &engine.inner.storage,
                        &engine.inner.scan_gate,
                    )
                })
                .collect::<Vec<_>>()
        });
        if engine.inner.scan_gate.is_paused() {
            queue.extend(dirs);
            continue;
        }
        let count_before_batch = count;
        for batch in &batches {
            for chunk in batch.files.chunks(256) {
                if engine.inner.scan_gate.is_paused() {
                    // Re-read after deletion; never retain pre-delete metadata across the pause.
                    count = count_before_batch;
                    queue.extend(dirs.iter().cloned());
                    continue 'scan;
                }
                write_found(engine, id, version, &generation, chunk, &mut count)?;
            }
        }
        for batch in batches {
            queue.extend(batch.dirs);
            failed.extend(batch.failed);
        }
    }
    if control.cancel.load(Ordering::SeqCst) {
        bail!("扫描已取消，保留已发现文件与上次索引。");
    }
    let _permit = engine.inner.scan_gate.enter();
    let final_meta = fs::metadata(&root)?;
    if identity(&root, &final_meta)? != root_identity {
        bail!("扫描期间根目录身份改变，保留待校验索引。");
    }
    finish_scan(
        engine,
        id,
        Completion {
            version,
            generation: &generation,
            failed: &failed,
            count,
            dirty: control.dirty.load(Ordering::SeqCst),
            watch_failed: input.watch && control.watch_failed.load(Ordering::SeqCst),
        },
    )
}

struct Completion<'a> {
    version: i64,
    generation: &'a str,
    failed: &'a [PathBuf],
    count: usize,
    dirty: bool,
    watch_failed: bool,
}
fn finish_scan(engine: &Engine, id: &str, snapshot: Completion<'_>) -> Result<()> {
    let Completion {
        version,
        generation,
        failed,
        count,
        dirty,
        watch_failed,
    } = snapshot;
    let mut c = engine.lock()?;
    let tx = c.transaction()?;
    let active: i64 = query_cached(
        &tx,
        "SELECT config_version FROM scopes WHERE id=?",
        [id],
        |r| r.get(0),
    )?;
    if active != version {
        bail!("文件夹配置已改变，正在重新扫描。");
    }
    let stale: Vec<(i64, Vec<u8>, Vec<u8>)> = {
        let mut s=tx.prepare("SELECT e.id,d.path,e.native_name FROM entries e JOIN directories d ON e.dir_id=d.id JOIN memberships m ON m.entry_id=e.id WHERE m.scope_id=? AND m.generation<>?")?;
        let values = s
            .query_map(params![id, generation], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        values
    };
    for (entry_id, dir, name) in stale {
        let path = PathBuf::from(decode(&dir)).join(decode(&name));
        if !failed.iter().any(|p| path.starts_with(p)) {
            execute_cached(
                &tx,
                "DELETE FROM memberships WHERE scope_id=? AND entry_id=?",
                params![id, entry_id],
            )?;
        }
    }
    db::cleanup(&tx)?;
    let message = if !failed.is_empty() {
        Some(format!(
            "{} 个路径无法完整扫描，保留其旧索引。",
            failed.len()
        ))
    } else if watch_failed {
        Some("实时监听不可用，请手动刷新。".into())
    } else {
        None
    };
    execute_cached(
        &tx,
        "UPDATE scopes SET freshness=?,last_scan=?,scanned=?,message=? WHERE id=?",
        params![
            if dirty {
                "dirty"
            } else if failed.is_empty() {
                "current"
            } else {
                "partial"
            },
            now(),
            count as i64,
            message,
            id
        ],
    )?;
    db::bump(&tx)?;
    tx.commit()?;
    Ok(())
}

fn write_found(
    engine: &Engine,
    id: &str,
    version: i64,
    generation: &str,
    chunk: &[Found],
    count: &mut usize,
) -> Result<()> {
    let mut c = engine.lock()?;
    let tx = c.transaction()?;
    let active: i64 = query_cached(
        &tx,
        "SELECT config_version FROM scopes WHERE id=?",
        [id],
        |r| r.get(0),
    )?;
    if active != version {
        bail!("文件夹配置已改变，正在重新扫描。");
    }
    for file in chunk {
        let parent = file.path.parent().unwrap();
        let name = file.path.file_name().unwrap();
        let name_text = name.to_string_lossy();
        execute_cached(
            &tx,
            "INSERT OR IGNORE INTO directories(path,display) VALUES(?,?)",
            params![encode(parent.as_os_str()), parent.to_string_lossy()],
        )?;
        let dir_id: i64 = query_cached(
            &tx,
            "SELECT id FROM directories WHERE path=?",
            [encode(parent.as_os_str())],
            |r| r.get(0),
        )?;
        let old: Option<(i64, String)> = query_cached(
            &tx,
            "SELECT id,identity FROM entries WHERE dir_id=? AND native_name=?",
            params![dir_id, encode(name)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
        let mut inherited = Vec::<(String, String)>::new();
        if let Some((old_id, old_identity)) = old {
            if old_identity != file.identity {
                {
                    let mut s =
                        tx.prepare("SELECT scope_id,generation FROM memberships WHERE entry_id=?")?;
                    inherited = s
                        .query_map([old_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?;
                }
                execute_cached(&tx, "DELETE FROM entries WHERE id=?", [old_id])?;
            }
        }
        let ext = model::extension(&name_text);
        execute_cached(&tx,"INSERT INTO entries(dir_id,native_name,name,extension,group_name,size,mtime,identity,attributes)
              VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(dir_id,native_name) DO UPDATE SET name=excluded.name,extension=excluded.extension,group_name=excluded.group_name,size=excluded.size,mtime=excluded.mtime,identity=excluded.identity,attributes=excluded.attributes",
              params![dir_id,encode(name),name_text,ext,model::group(&ext),file.meta.len().min(i64::MAX as u64) as i64,mtime(&file.meta),file.identity,attributes(&file.meta)])?;
        let entry_id: i64 = query_cached(
            &tx,
            "SELECT id FROM entries WHERE dir_id=? AND native_name=?",
            params![dir_id, encode(name)],
            |r| r.get(0),
        )?;
        for (scope, generation) in inherited {
            execute_cached(
                &tx,
                "INSERT OR IGNORE INTO memberships(scope_id,entry_id,generation) VALUES(?,?,?)",
                params![scope, entry_id, generation],
            )?;
        }
        execute_cached(&tx,"INSERT INTO memberships(scope_id,entry_id,generation) VALUES(?,?,?) ON CONFLICT(scope_id,entry_id) DO UPDATE SET generation=CASE WHEN excluded.generation='everything' THEN memberships.generation ELSE excluded.generation END",params![id,entry_id,generation])?;
    }
    *count += chunk.len();
    if generation != "everything" {
        execute_cached(
            &tx,
            "UPDATE scopes SET scanned=? WHERE id=?",
            params![*count as i64, id],
        )?;
    }
    db::bump(&tx)?;
    tx.commit()?;
    Ok(())
}

/// Everything only supplies candidates. Authorize and stat each one before indexing it.
pub(crate) fn index_candidates(
    engine: &Engine,
    id: &str,
    paths: &[PathBuf],
) -> Result<Vec<String>> {
    let (root, expected, version, input) = {
        let c = engine.lock()?;
        db::root(&c, id)?
    };
    if identity(&root, &fs::metadata(&root)?)? != expected {
        bail!("监控文件夹身份改变。");
    }
    let rules = Excludes::new(&input.excludes)?;
    let mut ids = Vec::new();
    let mut count = 0;
    for chunk in paths.chunks(128) {
        let _permit = engine.inner.scan_gate.enter();
        let mut fresh = Vec::new();
        for path in chunk {
            let checked = (|| -> anyhow::Result<Found> {
                let canonical = path.canonicalize()?;
                let relative = canonical.strip_prefix(&root)?;
                if canonical.starts_with(&engine.inner.storage)
                    || (!input.recursive && relative.components().count() != 1)
                    || rules.matches(relative)
                {
                    bail!("候选路径被排除。");
                }
                // Reject symlinks/junctions at every level of the supplied path.
                let mut ancestor = Some(path.as_path());
                while let Some(part) = ancestor {
                    let m = fs::symlink_metadata(part)?;
                    if m.file_type().is_symlink() || reparse(attributes(&m)) {
                        bail!("不跟随链接候选路径。");
                    }
                    if part.canonicalize()? == root {
                        break;
                    }
                    ancestor = part.parent();
                }
                let meta = fs::symlink_metadata(&canonical)?;
                if !meta.is_file() || placeholder(attributes(&meta)) {
                    bail!("只索引本地普通文件。");
                }
                let identity = identity(&canonical, &meta)?;
                Ok(Found {
                    path: canonical,
                    meta,
                    identity,
                })
            })();
            let Ok(file) = checked else {
                continue;
            };
            let existing = {
                let c = engine.lock()?;
                c.query_row("SELECT e.id FROM entries e JOIN directories d ON d.id=e.dir_id JOIN memberships m ON m.entry_id=e.id JOIN scopes s ON s.id=m.scope_id WHERE d.path=? AND e.native_name=? AND e.identity=? AND e.size=? AND e.mtime=? AND e.attributes=? AND m.scope_id=? AND s.config_version=?",params![encode(file.path.parent().unwrap().as_os_str()),encode(file.path.file_name().unwrap()),file.identity,file.meta.len() as i64,mtime(&file.meta),attributes(&file.meta),id,version],|r|r.get::<_,i64>(0)).optional()?
            };
            if let Some(existing) = existing {
                ids.push(existing.to_string());
            } else {
                fresh.push(file);
            }
        }
        if !fresh.is_empty() {
            write_found(engine, id, version, "everything", &fresh, &mut count)?;
            let c = engine.lock()?;
            for file in fresh {
                let entry:i64=c.query_row("SELECT e.id FROM entries e JOIN directories d ON d.id=e.dir_id WHERE d.path=? AND e.native_name=?",params![encode(file.path.parent().unwrap().as_os_str()),encode(file.path.file_name().unwrap())],|r|r.get(0))?;
                ids.push(entry.to_string());
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod candidate_tests {
    use super::*;
    use crate::model::{Query, ScopeInput};
    #[test]
    fn candidates_respect_scope_excludes_identity_and_do_not_churn_revision() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("excluded")).unwrap();
        fs::create_dir_all(root.join("nested/private")).unwrap();
        fs::write(root.join("nested/private/secret.txt"), "nested excluded").unwrap();
        fs::write(root.join("good.txt"), "good").unwrap();
        fs::write(root.join("excluded/secret.txt"), "excluded").unwrap();
        fs::write(temp.path().join("outside.txt"), "outside").unwrap();
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let scope = e
            .add_scope(ScopeInput {
                content_enabled: false,
                path: display_path(&root),
                recursive: true,
                watch: false,
                excludes: vec!["excluded".into(), "nested/private".into()],
            })
            .unwrap();
        let start = std::time::Instant::now();
        while e.summary().unwrap().scopes[0].freshness != "current" {
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let existing = e.query(&Query::default()).unwrap().entries[0].id.clone();
        let revision = e.summary().unwrap().revision;
        let paths = vec![
            root.join("good.txt").canonicalize().unwrap(),
            root.join("excluded/secret.txt").canonicalize().unwrap(),
            root.join("nested/private/secret.txt")
                .canonicalize()
                .unwrap(),
            temp.path().join("outside.txt").canonicalize().unwrap(),
            root.join("missing.txt"),
        ];
        let ids = index_candidates(&e, &scope, &paths).unwrap();
        assert_eq!(ids, vec![existing]);
        assert_eq!(e.summary().unwrap().revision, revision);
        assert_eq!(e.query(&Query::default()).unwrap().total, 1);
        fs::write(root.join("new.txt"), "new").unwrap();
        let ids =
            index_candidates(&e, &scope, &[root.join("new.txt").canonicalize().unwrap()]).unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(e.preview_text(&ids[0]).unwrap().text, "new");
    }
    #[test]
    fn pre_move_scan_cannot_remove_new_membership_or_hidden_state() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("keep.txt"), "keep content").unwrap();
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let input = |p: &Path| ScopeInput {
            content_enabled: false,
            path: display_path(p),
            recursive: true,
            watch: false,
            excludes: vec![],
        };
        e.add_scope(input(&source)).unwrap();
        let target_scope = e.add_scope(input(&target)).unwrap();
        let wait = || {
            let start = std::time::Instant::now();
            while e
                .summary()
                .unwrap()
                .scopes
                .iter()
                .any(|s| s.freshness != "current")
            {
                assert!(start.elapsed() < std::time::Duration::from_secs(10));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        wait();
        let id = e.query(&Query::default()).unwrap().entries[0].id.clone();
        e.set_hidden(std::slice::from_ref(&id), true).unwrap();
        let version = db::root(&e.lock().unwrap(), &target_scope).unwrap().2;
        // Snapshot represents a scan that already visited the previously empty target.
        let snapshot = Completion {
            version,
            generation: "before-move",
            failed: &[],
            count: 0,
            dirty: false,
            watch_failed: false,
        };
        let plan = e
            .preview_move(std::slice::from_ref(&id), &display_path(&target))
            .unwrap();
        assert_eq!(
            e.execute_move(&plan.operation.id).unwrap().operation.items[0].status,
            "succeeded"
        );
        assert!(finish_scan(&e, &target_scope, snapshot).is_err());
        wait();
        let files = e
            .query(&Query {
                hidden: true,
                scope_id: target_scope,
                ..Default::default()
            })
            .unwrap()
            .entries;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, id);
        assert_eq!(e.preview_text(&id).unwrap().text, "keep content");
    }
}
