use crate::scan_issues::{self, Failure};
use crate::{db, model, platform::*, rules::Excludes, Control, Engine};
use anyhow::{bail, Result};
use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension, Params, Row};
use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, SyncSender},
        Arc,
    },
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
    meta: crate::scan_entry::Metadata,
    identity: String,
    scopes: u32,
}
const READ_CHUNK: usize = 256;
const WRITE_CHUNK: usize = 2048;
enum ReadEvent {
    Files(Vec<Found>),
    Complete(PathBuf, Batch),
}
struct Batch {
    dirs: Vec<PathBuf>,
    failed: Vec<Failure>,
}
struct ProgressGuard<'a>(&'a Control);
impl Drop for ProgressGuard<'_> {
    fn drop(&mut self) {
        *self.0.progress.lock().unwrap() = None;
    }
}
fn progress(control: &Control, phase: &str, path: &Path, processed: u64, discovered: u64) {
    if let Some((_, p)) = control.progress.lock().unwrap().as_mut() {
        p.phase = phase.into();
        p.current_path = display_path(path);
        p.processed_directories += processed;
        p.discovered_directories += discovered;
    }
}
struct ReadScope {
    id: String,
    root: PathBuf,
    identity: String,
    version: i64,
    recursive: bool,
    watch: bool,
    rules: Excludes,
    control: Arc<Control>,
    valid: AtomicBool,
}
impl ReadScope {
    fn live(&self) -> bool {
        self.valid.load(Ordering::SeqCst)
            && !self.control.cancel.load(Ordering::SeqCst)
            && !self.control.removed.load(Ordering::SeqCst)
    }
    fn includes(&self, path: &Path, directory: bool) -> bool {
        self.inclusion(path) & if directory { 2 } else { 1 } != 0
    }
    // Determine file/directory eligibility together: exclusion matching can be
    // expensive and does not need to run twice for every directory entry.
    fn inclusion(&self, path: &Path) -> u8 {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return 0;
        };
        if relative.as_os_str().is_empty() {
            return 2;
        }
        if (!self.recursive && relative.components().count() != 1) || self.rules.matches(relative) {
            0
        } else if self.recursive {
            3
        } else {
            1
        }
    }
}
struct Reader<'a> {
    scopes: &'a [ReadScope],
    storage: &'a Path,
    gate: &'a crate::scan_gate::ScanGate,
    run: &'a crate::scan_progress::Run,
    limited: bool,
}
impl Reader<'_> {
    fn interrupted(&self) -> bool {
        self.gate.is_paused() || !self.scopes.iter().any(ReadScope::live)
    }
    fn masks(&self, path: &Path) -> (u32, u32) {
        let mut files = 0;
        let mut directories = 0;
        for (i, scope) in self.scopes.iter().enumerate() {
            if scope.live() {
                let inclusion = scope.inclusion(path);
                if inclusion & 1 != 0 {
                    files |= 1 << i;
                }
                if inclusion & 2 != 0 {
                    directories |= 1 << i;
                }
            }
        }
        (files, directories)
    }
    fn read(&self, dir: &Path, sender: Option<&SyncSender<ReadEvent>>) -> Batch {
        let mut batch = Batch {
            dirs: Vec::new(),
            failed: Vec::new(),
        };
        if self.interrupted() || self.masks(dir).1 == 0 {
            return batch;
        }
        let counting = sender.is_none();
        self.run
            .update(false, |r| r.current_path = display_path(dir));
        for scope in self
            .scopes
            .iter()
            .filter(|s| s.live() && s.includes(dir, true))
        {
            progress(
                &scope.control,
                if counting { "counting" } else { "scanning" },
                dir,
                0,
                0,
            );
        }
        let checked = (|| -> std::result::Result<(), Failure> {
            if self.limited && !self.scopes.iter().any(|s| safe_ancestors(dir, &s.root)) {
                return Err(Failure::new(
                    dir,
                    "directory",
                    "父目录已改变、变成链接或无法访问，无法安全核对。",
                ));
            }
            let meta = fs::symlink_metadata(dir)
                .map_err(|error| Failure::io(dir, "directory", "读取目录信息失败", &error))?;
            if !meta.is_dir() {
                return Err(Failure::new(
                    dir,
                    "directory",
                    "该路径已不再是文件夹，可能在扫描期间被替换。",
                ));
            }
            if meta.file_type().is_symlink() || reparse(attributes(&meta)) {
                return Err(Failure::new(
                    dir,
                    "directory",
                    "目录是符号链接或重解析点，当前扫描不跟随此类目录。",
                ));
            }
            let canonical = dir
                .canonicalize()
                .map_err(|error| Failure::io(dir, "directory", "解析目录路径失败", &error))?;
            if canonical.starts_with(self.storage)
                || !self
                    .scopes
                    .iter()
                    .any(|s| s.live() && canonical.starts_with(&s.root))
            {
                return Err(Failure::new(
                    dir,
                    "directory",
                    "目录的实际位置已改变或不在允许扫描的范围内。",
                ));
            }
            Ok(())
        })();
        if let Err(failure) = checked {
            // A deleted/replaced subtree is a successful empty observation only
            // when all surviving ancestors are still normal authorized folders.
            if self.limited
                && self
                    .scopes
                    .iter()
                    .any(|scope| absent_directory(dir, &scope.root))
            {
                return batch;
            }
            batch.failed.push(failure);
            return batch;
        }
        let iter = match crate::scan_entry::read_dir(dir) {
            Ok(iter) => iter,
            Err(error) => {
                batch
                    .failed
                    .push(Failure::io(dir, "directory", "列出目录内容失败", &error));
                return batch;
            }
        };
        let mut files = Vec::with_capacity(READ_CHUNK);
        let mut visited = 0;
        for item in iter {
            if self.interrupted() {
                break;
            }
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    batch
                        .failed
                        .push(Failure::io(dir, "directory", "读取目录条目失败", &error));
                    continue;
                }
            };
            let path = item.path();
            visited += 1;
            if visited == READ_CHUNK {
                self.run.update(true, |r| {
                    r.visited_entries += visited as u64;
                    r.current_path = display_path(&path);
                });
                visited = 0;
            }
            if path.starts_with(self.storage) {
                continue;
            }
            let (file_mask, dir_mask) = self.masks(&path);
            if file_mask | dir_mask == 0 {
                continue;
            }
            // Directory planning never opens files for identity or reads content.
            // file_type is supplied by enumeration on Windows and common Unix FSs.
            if counting {
                match item.file_type() {
                    Ok(kind) if !kind.is_dir() || kind.is_symlink() => continue,
                    Err(error) => {
                        batch
                            .failed
                            .push(Failure::io(&path, "path", "读取路径类型失败", &error));
                        continue;
                    }
                    _ => {}
                }
            }
            // This never follows symlinks, and reuses enumeration metadata on Windows.
            let meta = match item.metadata() {
                Ok(m) => m,
                Err(error) => {
                    batch
                        .failed
                        .push(Failure::io(&path, "path", "读取文件或目录信息失败", &error));
                    continue;
                }
            };
            let attrs = meta.attributes();
            if meta.kind().is_symlink() {
                continue;
            }
            if meta.is_dir() && dir_mask != 0 {
                if reparse(attrs) || placeholder(attrs) {
                    batch.failed.push(Failure::new(
                        &path,
                        "directory",
                        if placeholder(attrs) {
                            "云盘占位目录尚未完整下载到本机，当前扫描不会主动下载其内容。"
                        } else {
                            "目录是重解析点或链接，当前扫描不跟随此类目录。"
                        },
                    ));
                } else {
                    batch.dirs.push(path);
                }
            } else if !counting && meta.is_file() && file_mask != 0 {
                match meta.identity(&path) {
                    Ok(identity) => files.push(Found {
                        path,
                        meta,
                        identity,
                        scopes: file_mask,
                    }),
                    Err(error) => batch.failed.push(Failure::new(
                        &path,
                        "file",
                        format!("读取文件身份失败：{error:#}"),
                    )),
                }
            }
            if files.len() == READ_CHUNK
                && sender
                    .unwrap()
                    .send(ReadEvent::Files(std::mem::replace(
                        &mut files,
                        Vec::with_capacity(READ_CHUNK),
                    )))
                    .is_err()
            {
                return batch;
            }
        }
        if visited > 0 {
            self.run
                .update(true, |r| r.visited_entries += visited as u64);
        }
        if !files.is_empty() && !self.interrupted() {
            let _ = sender.unwrap().send(ReadEvent::Files(files));
        }
        batch
    }
}
#[derive(Default)]
pub(crate) struct ScanStats {
    pub directories: u64,
    pub files: u64,
}
pub(crate) fn run(engine: &Engine, id: &str, control: &Arc<Control>) -> Result<()> {
    run_group_with_reason(
        engine,
        &[(id.to_owned(), control.clone())],
        "文件夹变化或增量核对无法完成，需要完整核对",
    )
    .map(|_| ())
}
#[cfg(test)]
pub(crate) fn run_group(
    engine: &Engine,
    requested: &[(String, Arc<Control>)],
) -> Result<ScanStats> {
    run_group_with_reason(engine, requested, "手动刷新或启动核对")
}
pub(crate) fn run_group_with_reason(
    engine: &Engine,
    requested: &[(String, Arc<Control>)],
    reason: &str,
) -> Result<ScanStats> {
    run_group_limited(engine, requested, reason, None)
}
fn run_group_limited(
    engine: &Engine,
    requested: &[(String, Arc<Control>)],
    reason: &str,
    limits: Option<&[PathBuf]>,
) -> Result<ScanStats> {
    let pool = engine.inner.pool.lock().unwrap().clone();
    let run = engine
        .inner
        .scan_runs
        .lock()
        .unwrap()
        .start(requested.iter().map(|(id, _)| id.clone()).collect(), reason);
    let mut scopes = Vec::new();
    for (id, control) in requested {
        if control.cancel.load(Ordering::SeqCst) || control.removed.load(Ordering::SeqCst) {
            continue;
        }
        let root_info = { db::root(&*engine.lock()?, id) };
        let (root, expected, version, input) = match root_info {
            Ok(info) => info,
            Err(_) if control.removed.load(Ordering::SeqCst) => continue,
            Err(error) => return Err(error),
        };
        run.update(false, |r| r.current_path = display_path(&root));
        let meta = match fs::metadata(&root) {
            Ok(meta) => meta,
            Err(error) => {
                scan_issues::root_failure(
                    engine,
                    id,
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        "access_denied"
                    } else {
                        "offline"
                    },
                    Failure::io(&root, "directory", "读取监控文件夹失败", &error),
                )?;
                continue;
            }
        };
        let identity_failure = match identity(&root, &meta) {
            Ok(value) if value == expected => None,
            Ok(_) => Some(Failure::new(
                &root,
                "directory",
                "根目录身份发生变化，请检查路径后重新选择文件夹。",
            )),
            Err(error) => Some(Failure::new(
                &root,
                "directory",
                format!("读取文件夹身份失败：{error:#}"),
            )),
        };
        if let Some(failure) = identity_failure {
            scan_issues::root_failure(engine, id, "identity_changed", failure)?;
            continue;
        }
        scopes.push(ReadScope {
            id: id.clone(),
            root,
            identity: expected,
            version,
            recursive: input.recursive,
            watch: input.watch,
            rules: Excludes::new(&input.excludes)?,
            control: control.clone(),
            valid: true.into(),
        });
    }
    if scopes.len() > 32 {
        bail!("扫描范围过多");
    }
    let mut stats = ScanStats::default();
    if scopes.is_empty() {
        run.finish(
            if requested
                .iter()
                .all(|(_, c)| c.cancel.load(Ordering::SeqCst) || c.removed.load(Ordering::SeqCst))
            {
                "cancelled"
            } else {
                "partial"
            },
        );
        return Ok(stats);
    }
    let roots: Vec<_> = match limits {
        Some(paths) => minimal_roots(paths.iter().cloned()),
        None => scopes
            .iter()
            .map(|s| s.root.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
    };
    run.update(false, |r| r.discovered_directories = roots.len() as u64);
    let mut guards = Vec::new();
    for scope in &scopes {
        *scope.control.progress.lock().unwrap() = Some((
            std::time::Instant::now(),
            model::ScanProgress {
                phase: "counting".into(),
                processed_directories: 0,
                discovered_directories: roots.iter().filter(|p| scope.includes(p, true)).count()
                    as u64,
                current_path: display_path(&scope.root),
                elapsed_ms: 0,
            },
        ));
        guards.push(ProgressGuard(&scope.control));
    }
    let generation = uuid::Uuid::new_v4().to_string();
    let mut writes: Vec<_> = scopes
        .iter()
        .map(|s| WriteScope {
            id: &s.id,
            version: s.version,
            generation: &generation,
            plan: Some(s),
            count: 0,
        })
        .collect();
    {
        let mut c = engine.lock()?;
        let tx = c.transaction()?;
        for s in &scopes {
            if s.live() {
                if limits.is_none() {
                    db::set_state(&tx, &s.id, "available", "scanning", None)?;
                } else {
                    tx.execute("UPDATE scopes SET freshness='scanning' WHERE id=?", [&s.id])?;
                    db::bump(&tx)?;
                }
                tx.execute("UPDATE scopes SET scanned=0 WHERE id=?", [&s.id])?;
            }
        }
        tx.commit()?;
    }
    let reader = Reader {
        scopes: &scopes,
        storage: &engine.inner.storage,
        gate: &engine.inner.scan_gate,
        run: &run,
        limited: limits.is_some(),
    };
    let mut queue = VecDeque::from(roots.clone());
    let mut failed = Vec::new();
    let started = std::time::Instant::now();
    // Freeze a directory work list first. Only directory paths are retained, not
    // file metadata, so memory stays proportional to directory count.
    let mut planned: HashSet<PathBuf> = roots.iter().cloned().collect();
    let mut plan = VecDeque::new();
    while !queue.is_empty() {
        let _permit = engine.inner.scan_gate.enter();
        if !scopes.iter().any(ReadScope::live) {
            break;
        }
        if reader.interrupted() {
            continue;
        }
        let dirs: Vec<_> = (0..queue.len().min(64))
            .filter_map(|_| queue.pop_front())
            .collect();
        let batches = pool.install(|| {
            dirs.par_iter()
                .map(|dir| reader.read(dir, None))
                .collect::<Vec<_>>()
        });
        if engine.inner.scan_gate.is_paused() {
            queue.extend(dirs);
            continue;
        }
        plan.extend(dirs);
        let mut discovered = Vec::new();
        for batch in batches {
            // The actual scan retries these paths. Planning failures must not
            // duplicate (or outlive) the result of that second observation.
            for dir in batch.dirs {
                if planned.insert(dir.clone()) {
                    discovered.push(dir);
                }
            }
        }
        run.update(true, |r| r.discovered_directories = planned.len() as u64);
        for scope in scopes.iter().filter(|s| s.live()) {
            if let Some((_, p)) = scope.control.progress.lock().unwrap().as_mut() {
                p.discovered_directories += discovered
                    .iter()
                    .filter(|p| scope.includes(p, true))
                    .count() as u64;
            }
        }
        queue.extend(discovered);
    }
    if !scopes.iter().any(ReadScope::live) {
        run.finish("cancelled");
        return Ok(stats);
    }
    run.update(true, |r| {
        r.total_directories = Some(plan.len() as u64);
        r.phase = "scanning".into();
    });
    let counting_ms = started.elapsed().as_millis();
    let pipeline_started = std::time::Instant::now();
    queue = plan;
    let mut writing = std::time::Duration::ZERO;
    while !queue.is_empty() {
        let _permit = engine.inner.scan_gate.enter();
        if !scopes.iter().any(ReadScope::live) {
            break;
        }
        if engine.inner.scan_gate.is_paused() {
            continue;
        }
        // All fixed-plan directories are available to work stealing; a slow
        // member no longer blocks admission of the next group of 64 directories.
        let dirs: Vec<_> = queue.drain(..).collect();
        let counts: Vec<_> = writes.iter().map(|s| s.count).collect();
        let previous_files = stats.files;
        let previous_dirs = stats.directories;
        let batches = std::thread::scope(|threads| -> Result<Vec<Batch>> {
            let (sender, receiver) = sync_channel(pool.current_num_threads() * 2);
            let read = &reader;
            let read_dirs = &dirs;
            let pool = &pool;
            let producer = threads.spawn(move || {
                pool.install(|| {
                    read_dirs.par_iter().for_each(|dir| {
                        if read.interrupted() {
                            return;
                        }
                        let batch = read.read(dir, Some(&sender));
                        let _ = sender.send(ReadEvent::Complete(dir.clone(), batch));
                    })
                });
            });
            let mut files = Vec::with_capacity(WRITE_CHUNK);
            let mut completed = Vec::<PathBuf>::new();
            let mut batches = Vec::new();
            let mut flush = |files: &mut Vec<Found>, completed: &mut Vec<PathBuf>| -> Result<()> {
                if reader.interrupted() {
                    return Ok(());
                }
                if !files.is_empty() {
                    for scope in scopes
                        .iter()
                        .filter(|s| s.live() && s.includes(&files[0].path, false))
                    {
                        progress(
                            &scope.control,
                            "indexing",
                            files[0].path.parent().unwrap(),
                            0,
                            0,
                        );
                    }
                    let start = std::time::Instant::now();
                    run.update(false, |r| r.phase = "indexing".into());
                    write_batch(engine, &mut writes, files)?;
                    run.update(true, |r| r.checked_files += files.len() as u64);
                    writing += start.elapsed();
                    files.clear();
                }
                stats.directories += completed.len() as u64;
                run.update(!completed.is_empty(), |r| {
                    r.processed_directories = stats.directories;
                    r.phase = "scanning".into();
                });
                for scope in scopes.iter().filter(|s| s.live()) {
                    if let Some(path) = completed.iter().rev().find(|p| scope.includes(p, true)) {
                        progress(
                            &scope.control,
                            "scanning",
                            path,
                            completed.iter().filter(|p| scope.includes(p, true)).count() as u64,
                            0,
                        );
                    }
                }
                completed.clear();
                Ok(())
            };
            let mut received_files = 0u64;
            for event in receiver {
                if reader.interrupted() {
                    break;
                }
                match event {
                    ReadEvent::Files(chunk) => {
                        received_files += chunk.len() as u64;
                        files.extend(chunk);
                    }
                    ReadEvent::Complete(path, batch) => {
                        completed.push(path);
                        batches.push(batch);
                    }
                }
                if files.len() >= WRITE_CHUNK || completed.len() >= 64 {
                    flush(&mut files, &mut completed)?;
                }
            }
            flush(&mut files, &mut completed)?;
            producer.join().expect("scan reader panicked");
            stats.files += received_files;
            Ok(batches)
        })?;
        if engine.inner.scan_gate.is_paused() {
            for (scope, count) in writes.iter_mut().zip(counts) {
                scope.count = count;
            }
            queue.extend(dirs);
            stats.files = previous_files;
            stats.directories = previous_dirs;
            run.update(false, |r| {
                r.checked_files = previous_files;
                r.processed_directories = previous_dirs;
            });
            for scope in &scopes {
                if let Some((_, p)) = scope.control.progress.lock().unwrap().as_mut() {
                    p.processed_directories = 0;
                }
            }
            continue;
        }
        for batch in batches {
            failed.extend(batch.dirs.into_iter().filter(|p| !planned.contains(p)).map(|path|
                Failure::new(&path, "directory", "扫描期间新出现或恢复可访问的目录，尚未纳入本次扫描计划，请刷新后核对。")
            ));
            failed.extend(batch.failed);
        }
    }
    let pipeline_ms = pipeline_started.elapsed().as_millis();
    let finalize_started = std::time::Instant::now();
    let _permit = engine.inner.scan_gate.enter();
    run.update(false, |r| r.phase = "finalizing".into());
    let mut incomplete =
        !failed.is_empty() || scopes.len() != requested.len() || scopes.iter().any(|s| !s.live());
    for (scope, write) in scopes.iter().zip(&writes).filter(|(s, _)| s.live()) {
        let result = (|| -> Result<()> {
            if !fs::metadata(&scope.root)
                .is_ok_and(|m| identity(&scope.root, &m).is_ok_and(|i| i == scope.identity))
            {
                bail!("扫描期间根目录身份改变，保留待校验索引。");
            }
            let failed: Vec<_> = failed
                .iter()
                .filter(|failure| {
                    scope.includes(&failure.path, true) || scope.includes(&failure.path, false)
                })
                .cloned()
                .collect();
            progress(&scope.control, "finalizing", &scope.root, 0, 0);
            finish_scan(
                engine,
                &scope.id,
                Completion {
                    version: scope.version,
                    generation: &generation,
                    failed: &failed,
                    count: write.count,
                    dirty: scope.control.dirty.load(Ordering::SeqCst),
                    watch_failed: scope.watch && scope.control.watch_failed.load(Ordering::SeqCst),
                    control: Some(&scope.control),
                    limits,
                    defer_cleanup: false,
                },
            )
        })();
        if let Err(error) = result {
            incomplete = true;
            if scope.live() {
                scan_issues::root_failure(
                    engine,
                    &scope.id,
                    "available",
                    Failure::new(
                        &scope.root,
                        "directory",
                        format!("完成目录核对失败：{error:#}"),
                    ),
                )?;
            }
        }
    }
    {
        let mut c = engine.lock()?;
        let tx = c.transaction()?;
        db::cleanup(&tx)?;
        tx.commit()?;
    }
    run.finish(if scopes.iter().all(|s| !s.live()) {
        "cancelled"
    } else if incomplete {
        "partial"
    } else {
        "complete"
    });
    if std::env::var_os("FILEM_SCAN_PROFILE").is_some() {
        eprintln!(
            "FILEM_SCAN_PROFILE {}",
            serde_json::json!({"scopes": scopes.len(), "directories": stats.directories,
            "files": stats.files, "elapsed_ms": started.elapsed().as_millis(), "write_ms": writing.as_millis(), "counting_ms":counting_ms, "pipeline_ms":pipeline_ms, "finalize_ms":finalize_started.elapsed().as_millis()})
        );
    }
    Ok(stats)
}

struct Completion<'a> {
    limits: Option<&'a [PathBuf]>,
    defer_cleanup: bool,
    version: i64,
    generation: &'a str,
    failed: &'a [Failure],
    count: usize,
    dirty: bool,
    watch_failed: bool,
    control: Option<&'a Control>,
}
fn finish_scan(engine: &Engine, id: &str, snapshot: Completion<'_>) -> Result<()> {
    let Completion {
        limits,
        defer_cleanup,
        version,
        generation,
        failed,
        count,
        dirty,
        watch_failed,
        control,
    } = snapshot;
    let mut c = engine.lock()?;
    let tx = c.transaction()?;
    if control.is_some_and(|c| c.cancel.load(Ordering::SeqCst) || c.removed.load(Ordering::SeqCst))
    {
        return Ok(());
    }
    let active: i64 = query_cached(
        &tx,
        "SELECT config_version FROM scopes WHERE id=?",
        [id],
        |r| r.get(0),
    )?;
    if active != version {
        bail!("文件夹配置已改变，正在重新扫描。");
    }
    let mut sql = "SELECT e.id,d.path,e.native_name FROM entries e JOIN directories d ON e.dir_id=d.id JOIN memberships m ON m.entry_id=e.id WHERE m.scope_id=? AND m.generation<>?".to_string();
    let mut values = vec![
        rusqlite::types::Value::Text(id.into()),
        rusqlite::types::Value::Text(generation.into()),
    ];
    if let Some(paths) = limits {
        let mut ranges = Vec::new();
        for path in paths {
            let (lower, upper) = directory_bounds(path);
            ranges.push("(d.path=? OR (d.path>=? AND d.path<?) OR (d.path=? AND e.native_name=?))");
            values.extend([encode(path.as_os_str()).into(), lower.into(), upper.into()]);
            // The subtree root may previously have been a regular file.
            values.push(encode(path.parent().unwrap().as_os_str()).into());
            values.push(encode(path.file_name().unwrap()).into());
        }
        if ranges.is_empty() {
            return Ok(());
        }
        sql.push_str(&format!(" AND ({})", ranges.join(" OR ")));
    }
    let stale: Vec<(i64, Vec<u8>, Vec<u8>)> = {
        let mut statement = tx.prepare(&sql)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(values), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    let failed_set: HashSet<_> = failed
        .iter()
        .map(|failure| failure.path.as_path())
        .collect();
    for (entry_id, dir, name) in stale {
        let path = PathBuf::from(decode(&dir)).join(decode(&name));
        if !path
            .ancestors()
            .any(|ancestor| failed_set.contains(ancestor))
        {
            execute_cached(
                &tx,
                "DELETE FROM memberships WHERE scope_id=? AND entry_id=?",
                params![id, entry_id],
            )?;
        }
    }
    if !defer_cleanup {
        db::cleanup(&tx)?;
    }
    let previous_message: Option<String> = if limits.is_some() {
        tx.query_row("SELECT message FROM scopes WHERE id=?", [id], |r| r.get(0))?
    } else {
        None
    };
    let previous_issue_count = scan_issues::count(&tx, id)?;
    scan_issues::replace(&tx, id, limits, failed)?;
    let issue_count = scan_issues::count(&tx, id)?;
    let message = if issue_count > 0 {
        Some(scan_issues::summary(issue_count))
    } else if watch_failed {
        Some("实时监听不可用，请手动刷新。".into())
    } else {
        previous_message
            .filter(|message| previous_issue_count == 0 || !scan_issues::is_summary(message))
    };
    let count = if limits.is_some() {
        tx.query_row(
            "SELECT COUNT(*) FROM memberships WHERE scope_id=?",
            [id],
            |r| r.get::<_, i64>(0),
        )? as usize
    } else {
        count
    };
    execute_cached(
        &tx,
        "UPDATE scopes SET freshness=?,last_scan=?,scanned=?,message=? WHERE id=?",
        params![
            if control.map_or(dirty, |c| c.dirty.load(Ordering::SeqCst)) {
                "dirty"
            } else if issue_count == 0 && message.is_none() {
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

struct WriteScope<'a> {
    id: &'a str,
    version: i64,
    generation: &'a str,
    plan: Option<&'a ReadScope>,
    count: usize,
}
fn write_batch(engine: &Engine, scopes: &mut [WriteScope<'_>], chunk: &[Found]) -> Result<()> {
    let mut c = engine.lock()?;
    let tx = c.transaction()?;
    let mut active = 0u32;
    for (i, scope) in scopes.iter().enumerate() {
        if scope.plan.is_some_and(|p| !p.live()) {
            continue;
        }
        let version: Option<i64> = query_cached(
            &tx,
            "SELECT config_version FROM scopes WHERE id=?",
            [scope.id],
            |r| r.get(0),
        )
        .optional()?;
        if version != Some(scope.version) {
            if let Some(plan) = scope.plan {
                plan.valid.store(false, Ordering::SeqCst);
                if version.is_some() {
                    db::set_state(
                        &tx,
                        scope.id,
                        "available",
                        "partial",
                        Some("文件夹配置已改变，等待新的扫描请求。"),
                    )?;
                }
                continue;
            }
            bail!("文件夹配置已改变，正在重新扫描。");
        }
        active |= 1 << i;
    }
    let files: Vec<_> = chunk.iter().filter(|f| f.scopes & active != 0).collect();
    let ids = upsert_entries(&tx, &files)?;
    let mut counts = vec![0; scopes.len()];
    {
        let mut membership = tx.prepare_cached("INSERT INTO memberships(scope_id,entry_id,generation) VALUES(?,?,?) ON CONFLICT(scope_id,entry_id) DO UPDATE SET generation=excluded.generation WHERE excluded.generation<>'everything' AND memberships.generation<>excluded.generation")?;
        for (file, entry) in files.iter().zip(ids) {
            for (i, scope) in scopes.iter().enumerate() {
                if file.scopes & active & (1 << i) != 0 {
                    membership.execute(params![scope.id, entry, scope.generation])?;
                    counts[i] += 1;
                }
            }
        }
    }
    for (i, scope) in scopes.iter().enumerate() {
        if active & (1 << i) != 0 && scope.generation != "everything" {
            execute_cached(
                &tx,
                "UPDATE scopes SET scanned=? WHERE id=?",
                params![(scope.count + counts[i]) as i64, scope.id],
            )?;
        }
    }
    if active != 0 {
        db::bump(&tx)?;
    }
    tx.commit()?;
    for (scope, count) in scopes.iter_mut().zip(counts) {
        scope.count += count;
    }
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
    let mut scopes = [WriteScope {
        id,
        version,
        generation,
        plan: None,
        count: *count,
    }];
    write_batch(engine, &mut scopes, chunk)?;
    *count = scopes[0].count;
    Ok(())
}

fn upsert_entries(tx: &Connection, chunk: &[&Found]) -> Result<Vec<i64>> {
    let mut ids = Vec::with_capacity(chunk.len());
    let hidden_roots = crate::hidden::roots(tx)?;
    {
        // Keep prepared statements checked out for the transaction rather than
        // repeatedly hashing long SQL strings and returning them to the cache.
        let mut find_directory = tx.prepare_cached("SELECT id FROM directories WHERE path=?")?;
        let mut add_directory =
            tx.prepare_cached("INSERT INTO directories(path,display,hidden) VALUES(?,?,?)")?;
        let mut find_entry = tx.prepare_cached(
            "SELECT id,identity,name=?3 AND extension=?4 AND group_name=?5 AND size=?6 AND mtime=?7 AND attributes=?8 FROM entries WHERE dir_id=?1 AND native_name=?2"
        )?;
        let mut insert_entry = tx.prepare_cached(
            "INSERT INTO entries(dir_id,native_name,name,extension,group_name,size,mtime,identity,attributes) VALUES(?,?,?,?,?,?,?,?,?)"
        )?;
        let mut update_entry = tx.prepare_cached(
            "UPDATE entries SET name=?,extension=?,group_name=?,size=?,mtime=?,attributes=? WHERE id=?"
        )?;
        let mut directory = None;
        for file in chunk {
            let parent = file.path.parent().unwrap();
            let name = encode(file.path.file_name().unwrap());
            let name_text = file.path.file_name().unwrap().to_string_lossy();
            let ext = model::extension(&name_text);
            let group = model::group(&ext);
            let size = file.meta.len().min(i64::MAX as u64) as i64;
            let time = file.meta.modified();
            let attrs = file.meta.attributes();
            let dir_id =
                if let Some((cached_path, cached_id)) = directory.filter(|(p, _)| *p == parent) {
                    directory = Some((cached_path, cached_id));
                    cached_id
                } else {
                    let native = encode(parent.as_os_str());
                    let existing: Option<i64> = find_directory
                        .query_row([&native], |r| r.get(0))
                        .optional()?;
                    let dir_id = match existing {
                        Some(id) => id,
                        None => {
                            add_directory.execute(params![
                                native,
                                parent.to_string_lossy(),
                                crate::hidden::contains(&hidden_roots, parent)
                            ])?;
                            tx.last_insert_rowid()
                        }
                    };
                    directory = Some((parent, dir_id));
                    dir_id
                };
            let old: Option<(i64, String, bool)> = find_entry
                .query_row(
                    params![dir_id, name, name_text, ext, group, size, time, attrs],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let mut inherited = Vec::<(String, String)>::new();
            let existing = match old {
                Some((old_id, old_identity, unchanged)) if old_identity == file.identity => {
                    if !unchanged {
                        update_entry
                            .execute(params![name_text, ext, group, size, time, attrs, old_id])?;
                    }
                    Some(old_id)
                }
                Some((old_id, _, _)) => {
                    let mut memberships = tx.prepare_cached(
                        "SELECT scope_id,generation FROM memberships WHERE entry_id=?",
                    )?;
                    inherited = memberships
                        .query_map([old_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?;
                    execute_cached(tx, "DELETE FROM entries WHERE id=?", [old_id])?;
                    None
                }
                None => None,
            };
            let entry_id = match existing {
                Some(id) => id,
                None => {
                    insert_entry.execute(params![
                        dir_id,
                        name,
                        name_text,
                        ext,
                        group,
                        size,
                        time,
                        file.identity,
                        attrs
                    ])?;
                    tx.last_insert_rowid()
                }
            };
            for (scope, generation) in inherited {
                execute_cached(
                    tx,
                    "INSERT OR IGNORE INTO memberships(scope_id,entry_id,generation) VALUES(?,?,?)",
                    params![scope, entry_id, generation],
                )?;
            }
            ids.push(entry_id);
        }
    }
    Ok(ids)
}

fn minimal_roots(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort_by_key(|path| path.components().count());
    let mut selected = HashSet::new();
    let mut roots = Vec::new();
    for path in paths {
        if !path.ancestors().any(|parent| selected.contains(parent)) {
            selected.insert(path.clone());
            roots.push(path);
        }
    }
    roots
}
fn directory_bounds(path: &Path) -> (Vec<u8>, Vec<u8>) {
    let mut prefix = path.as_os_str().to_os_string();
    prefix.push(std::path::MAIN_SEPARATOR_STR);
    let lower = encode(&prefix);
    let mut upper = lower.clone();
    *upper.last_mut().unwrap() += 1;
    (lower, upper)
}
fn safe_ancestors(path: &Path, root: &Path) -> bool {
    if !path.starts_with(root) {
        return false;
    }
    for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
        if !ancestor.starts_with(root) {
            break;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta)
                if meta.is_dir()
                    && !meta.file_type().is_symlink()
                    && !reparse(attributes(&meta)) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return false,
        }
    }
    true
}
fn absent_directory(path: &Path, root: &Path) -> bool {
    if path == root || !safe_ancestors(path, root) {
        return false;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => meta.is_file() && !meta.file_type().is_symlink() && !reparse(attributes(&meta)),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Reconcile known file paths without restarting traversal of the entire scope.
/// Directory events reconcile only their affected subtrees; uncertain roots still recover fully.
pub(crate) fn update_files(
    engine: &Engine,
    id: &str,
    control: &Arc<Control>,
    paths: &[PathBuf],
) -> Result<bool> {
    let (root, expected, version, input) = {
        let c = engine.lock()?;
        let usable: bool = c.query_row(
            "SELECT availability='available' AND last_scan IS NOT NULL AND freshness IN ('current','dirty','partial') FROM scopes WHERE id=?",
            [id], |r| r.get(0),
        )?;
        if !usable {
            return Ok(false);
        }
        db::root(&c, id)?
    };
    if !fs::metadata(&root).is_ok_and(|m| identity(&root, &m).is_ok_and(|v| v == expected)) {
        return Ok(false);
    }
    let rules = Excludes::new(&input.excludes)?;
    let mut present = Vec::new();
    let mut missing = Vec::new();
    let mut subtrees = Vec::new();
    for path in paths {
        let Ok(relative) = path.strip_prefix(&root) else {
            return Ok(false);
        };
        if path.starts_with(&engine.inner.storage)
            || rules.matches(relative)
            || (!input.recursive && relative.components().count() != 1)
        {
            continue;
        }
        match fs::symlink_metadata(path) {
            Ok(meta)
                if meta.is_file()
                    && !meta.file_type().is_symlink()
                    && !reparse(attributes(&meta))
                    && !placeholder(attributes(&meta)) =>
            {
                let (lower, upper) = directory_bounds(path);
                let c = engine.lock()?;
                let was_directory: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM directories WHERE path=? OR (path>=? AND path<?))",
                    params![encode(path.as_os_str()), lower, upper],
                    |r| r.get(0),
                )?;
                if was_directory {
                    subtrees.push(path.clone());
                }
                present.push(path.clone())
            }
            Ok(meta)
                if meta.is_dir()
                    && !meta.file_type().is_symlink()
                    && !reparse(attributes(&meta))
                    && !placeholder(attributes(&meta)) =>
            {
                if path == &root {
                    return Ok(false);
                }
                subtrees.push(path.clone());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Native byte prefixes preserve path boundaries on Unix and Windows.
                let mut prefix = path.as_os_str().to_os_string();
                prefix.push(std::path::MAIN_SEPARATOR_STR);
                let lower = encode(&prefix);
                let mut upper = lower.clone();
                *upper.last_mut().unwrap() += 1;
                let c = engine.lock()?;
                let directory: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM directories WHERE path=? OR (path>=? AND path<?))",
                    params![encode(path.as_os_str()), lower, upper],
                    |r| r.get(0),
                )?;
                if path == &root {
                    return Ok(false);
                }
                if directory {
                    subtrees.push(path.clone());
                } else {
                    missing.push(path);
                }
            }
            _ => return Ok(false),
        }
    }
    if control.cancel.load(Ordering::SeqCst) || control.removed.load(Ordering::SeqCst) {
        bail!("扫描已取消，保留已发现文件与上次索引。");
    }
    let subtrees = minimal_roots(subtrees);
    // Reuse the same fixed-plan scanner and scoped completion checks, but never
    // touch generations or files in an unrelated subtree.
    for roots in subtrees.chunks(128) {
        run_group_limited(
            engine,
            &[(id.to_owned(), control.clone())],
            "目录变化，核对受影响子目录",
            Some(roots),
        )?;
    }
    if control.cancel.load(Ordering::SeqCst) || control.removed.load(Ordering::SeqCst) {
        bail!("扫描已取消");
    }
    present.retain(|path| {
        !subtrees
            .iter()
            .any(|root| path != root && path.starts_with(root))
    });
    let indexed = index_candidates(engine, id, &present)?;
    let _permit = engine.inner.scan_gate.enter();
    let mut c = engine.lock()?;
    let tx = c.transaction()?;
    if db::root(&tx, id)?.2 != version || control.cancel.load(Ordering::SeqCst) {
        bail!("文件夹配置已改变或扫描已取消。");
    }
    let previous_issue_count = scan_issues::count(&tx, id)?;
    for entry_id in indexed {
        let path = db::entry_path(&tx, &entry_id)?.0;
        tx.execute(
            "DELETE FROM scan_issues WHERE scope_id=? AND path=?",
            params![id, encode(path.as_os_str())],
        )?;
    }
    for path in missing {
        if !fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
            return Ok(false);
        }
        let row: Option<(i64, i64)> = tx.query_row(
            "SELECT e.id,e.dir_id FROM entries e JOIN directories d ON d.id=e.dir_id WHERE d.path=? AND e.native_name=?",
            params![encode(path.parent().unwrap().as_os_str()),encode(path.file_name().unwrap())],
            |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        if let Some((entry, dir)) = row {
            tx.execute(
                "DELETE FROM memberships WHERE scope_id=? AND entry_id=?",
                params![id, entry],
            )?;
            tx.execute("DELETE FROM entries WHERE id=? AND NOT EXISTS(SELECT 1 FROM memberships WHERE entry_id=?)", params![entry,entry])?;
            tx.execute("DELETE FROM directories WHERE id=? AND NOT EXISTS(SELECT 1 FROM entries WHERE dir_id=?)", params![dir,dir])?;
        }
        tx.execute(
            "DELETE FROM scan_issues WHERE scope_id=? AND path=?",
            params![id, encode(path.as_os_str())],
        )?;
    }
    let message: Option<String> =
        tx.query_row("SELECT message FROM scopes WHERE id=?", [id], |r| r.get(0))?;
    if previous_issue_count > 0 && message.as_deref().is_some_and(scan_issues::is_summary) {
        let count = scan_issues::count(&tx, id)?;
        tx.execute(
            "UPDATE scopes SET message=? WHERE id=?",
            params![(count > 0).then(|| scan_issues::summary(count)), id],
        )?;
    }
    tx.execute(
        "UPDATE scopes SET freshness=CASE WHEN ? THEN 'dirty' WHEN message IS NOT NULL THEN 'partial' ELSE 'current' END WHERE id=?",
        params![control.dirty.load(Ordering::SeqCst), id],
    )?;
    db::bump(&tx)?;
    tx.commit()?;
    Ok(true)
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
                    meta: crate::scan_entry::Metadata::Standard(meta),
                    identity,
                    scopes: 1,
                })
            })();
            let Ok(file) = checked else {
                continue;
            };
            let existing = {
                let c = engine.lock()?;
                c.query_row("SELECT e.id FROM entries e JOIN directories d ON d.id=e.dir_id JOIN memberships m ON m.entry_id=e.id JOIN scopes s ON s.id=m.scope_id WHERE d.path=? AND e.native_name=? AND e.identity=? AND e.size=? AND e.mtime=? AND e.attributes=? AND m.scope_id=? AND s.config_version=?",params![encode(file.path.parent().unwrap().as_os_str()),encode(file.path.file_name().unwrap()),file.identity,file.meta.len() as i64,file.meta.modified(),file.meta.attributes(),id,version],|r|r.get::<_,i64>(0)).optional()?
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
#[path = "scan_tests.rs"]
mod grouped_tests;

#[cfg(test)]
mod candidate_tests {
    use super::*;
    use crate::model::{Query, ScopeInput};
    #[test]
    fn file_events_update_only_affected_paths_and_preserve_partial_scan_warnings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("nested/deep")).unwrap();
        fs::write(root.join("stable.txt"), "keep").unwrap();
        fs::write(root.join("change.txt"), "old").unwrap();
        fs::write(root.join("nested/deep/gone.txt"), "gone").unwrap();
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let id = e
            .add_scope(ScopeInput {
                path: display_path(&root),
                recursive: true,
                watch: false,
                excludes: vec!["*.tmp".into()],
                content_enabled: false,
            })
            .unwrap();
        let start = std::time::Instant::now();
        while e.summary().unwrap().scopes[0].freshness != "current" {
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let root = root.canonicalize().unwrap();
        let control = e.inner.controls.lock().unwrap()[&id].clone();
        let stable = e
            .query(&Query {
                search: "stable.txt".into(),
                ..Default::default()
            })
            .unwrap()
            .entries[0]
            .id
            .clone();
        e.set_hidden(std::slice::from_ref(&stable), true).unwrap();
        let generation: String = e
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM memberships WHERE entry_id=?",
                [&stable],
                |r| r.get(0),
            )
            .unwrap();
        let last_scan = e.summary().unwrap().scopes[0].last_scan;
        fs::write(root.join("change.txt"), "a longer changed file").unwrap();
        fs::write(root.join("new.txt"), "new").unwrap();
        fs::write(root.join("excluded.tmp"), "excluded").unwrap();
        fs::remove_file(root.join("nested/deep/gone.txt")).unwrap();
        assert!(update_files(
            &e,
            &id,
            &control,
            &[
                root.join("change.txt"),
                root.join("new.txt"),
                root.join("excluded.tmp"),
                root.join("nested/deep/gone.txt"),
            ]
        )
        .unwrap());
        let results = e.query(&Query::default()).unwrap();
        assert_eq!(results.total, 2);
        assert_eq!(
            results
                .entries
                .iter()
                .find(|v| v.name == "change.txt")
                .unwrap()
                .size,
            21
        );
        assert_eq!(e.summary().unwrap().scopes[0].last_scan, last_scan);
        let after: String = e
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM memberships WHERE entry_id=?",
                [&stable],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(generation, after);
        assert_eq!(
            e.query(&Query {
                hidden: true,
                ..Default::default()
            })
            .unwrap()
            .entries[0]
                .id,
            stable
        );

        e.lock()
            .unwrap()
            .execute(
                "UPDATE scopes SET freshness='dirty',message='部分路径不可访问' WHERE id=?",
                [&id],
            )
            .unwrap();
        assert!(update_files(&e, &id, &control, &[root.join("new.txt")]).unwrap());
        let scope = e.summary().unwrap().scopes.remove(0);
        assert_eq!(scope.freshness, "partial");
        assert_eq!(scope.message.as_deref(), Some("部分路径不可访问"));
    }
    #[test]
    fn directory_rename_reconciles_only_old_and_new_subtrees() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("old/deep")).unwrap();
        fs::write(root.join("old/deep/file.txt"), "keep").unwrap();
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let id = e
            .add_scope(ScopeInput {
                path: display_path(&root),
                recursive: true,
                watch: false,
                excludes: vec![],
                content_enabled: false,
            })
            .unwrap();
        let start = std::time::Instant::now();
        while e.summary().unwrap().scopes[0].freshness != "current" {
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let root = root.canonicalize().unwrap();
        let control = e.inner.controls.lock().unwrap()[&id].clone();
        fs::rename(root.join("old"), root.join("new")).unwrap();
        assert!(update_files(&e, &id, &control, &[root.join("old")]).unwrap());
        assert_eq!(e.query(&Query::default()).unwrap().total, 0);
        assert!(update_files(&e, &id, &control, &[root.join("new")]).unwrap());
        assert_eq!(e.query(&Query::default()).unwrap().total, 1);
    }
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
            limits: None,
            defer_cleanup: false,
            version,
            generation: "before-move",
            failed: &[],
            count: 0,
            dirty: false,
            watch_failed: false,
            control: None,
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
