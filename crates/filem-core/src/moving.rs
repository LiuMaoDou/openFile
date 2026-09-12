use crate::{db, model::*, platform::*, rules::Excludes, Engine};
use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension};
#[cfg(unix)]
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::io::{Read, Write};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

struct Item {
    id: String,
    path: PathBuf,
    identity: String,
    size: u64,
    time: i64,
    ordinal: usize,
}
fn destination(path: &str, storage: &Path) -> Result<PathBuf> {
    let root = checked_root(Path::new(path)).context("目标文件夹不可用")?;
    if root.starts_with(storage) {
        bail!("不能移动到 FileM 的内部数据文件夹。");
    }
    Ok(root)
}
fn target_path(source: &Path, root: &Path) -> Result<PathBuf> {
    let target = root.join(source.file_name().context("文件名无效")?);
    if target == source {
        bail!("文件已经在目标文件夹中。");
    }
    match fs::symlink_metadata(&target) {
        Ok(_) => bail!("目标文件夹已有同名文件，将跳过，不会覆盖。"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(target),
        Err(e) => Err(e).context("目标位置不可访问"),
    }
}
impl Engine {
    pub fn preview_move(&self, ids: &[String], target: &str) -> Result<MovePlan> {
        if ids.is_empty() || ids.len() > 1000 {
            bail!("请选择 1 到 1000 个文件。");
        }
        let root = destination(target, &self.inner.storage)?;
        let root_identity = identity(&root, &fs::metadata(&root)?)?;
        let id = uuid::Uuid::new_v4().to_string();
        let created = now();
        let mut seen = HashSet::new();
        let mut targets = HashSet::new();
        let mut items = Vec::new();
        for entry in ids.iter().filter(|id| seen.insert((*id).clone())) {
            let raw = {
                let c = self.lock()?;
                db::entry_path(&c, entry)
            };
            let (path, identity, size, time, _) = raw.unwrap_or_default();
            let issue = self
                .checked_entry(entry)
                .and_then(|(checked, _, _, _)| {
                    if !fs::symlink_metadata(&checked)?.is_file() {
                        bail!("只支持移动普通文件。");
                    }
                    let target = target_path(&checked, &root)?;
                    #[cfg(windows)]
                    let key = display_path(&target).to_lowercase();
                    #[cfg(not(windows))]
                    let key = display_path(&target);
                    if !targets.insert(key) {
                        bail!("所选文件中存在同名文件，只移动首个，其他项跳过。");
                    }
                    Ok(())
                })
                .err()
                .map(|e| format!("{e:#}"));
            items.push((entry, path, identity, size, time, issue));
        }
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM move_operations WHERE state='planned' AND expires<?",
            [created],
        )?;
        tx.execute("INSERT INTO move_operations(id,created,expires,state,destination,destination_identity) VALUES(?,?,?,'planned',?,?)",params![id,created,created+600000,encode(root.as_os_str()),root_identity])?;
        for (ordinal, (entry, path, identity, size, time, issue)) in items.into_iter().enumerate() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            tx.execute("INSERT INTO move_items(operation_id,ordinal,entry_id,name,path,identity,size,mtime,status,message) VALUES(?,?,?,?,?,?,?,?,?,?)",params![id,ordinal as i64,entry,name,encode(path.as_os_str()),identity,size as i64,time,if issue.is_some(){"blocked"}else{"ready"},issue])?;
        }
        tx.commit()?;
        drop(c);
        self.move_plan(&id)
    }
    pub fn move_plan(&self, id: &str) -> Result<MovePlan> {
        let c = self.lock()?;
        let (created, expires, state, target): (i64, i64, String, Vec<u8>) = c.query_row(
            "SELECT created,expires,state,destination FROM move_operations WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let mut statement=c.prepare("SELECT entry_id,name,path,size,status,message FROM move_items WHERE operation_id=? ORDER BY ordinal")?;
        let items = statement
            .query_map([id], |r| {
                Ok(DeleteItem {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: display_path(&PathBuf::from(decode(&r.get::<_, Vec<u8>>(2)?))),
                    size: r.get(3)?,
                    status: r.get(4)?,
                    message: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(MovePlan {
            operation: DeletePlan {
                id: id.into(),
                created,
                expires,
                state,
                items,
            },
            destination: display_path(&PathBuf::from(decode(&target))),
        })
    }
    pub fn move_history(&self) -> Result<Vec<MovePlan>> {
        let ids = {
            let c = self.lock()?;
            let mut s=c.prepare("SELECT id FROM move_operations WHERE state<>'planned' ORDER BY created DESC LIMIT 10")?;
            let ids = s
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };
        ids.iter().map(|id| self.move_plan(id)).collect()
    }
    pub fn cancel_move(&self, id: &str) -> Result<()> {
        self.lock()?.execute(
            "UPDATE move_operations SET cancel=1 WHERE id=? AND state='running'",
            [id],
        )?;
        Ok(())
    }
    fn record_move(&self, id: &str, ordinal: usize, status: &str, message: &str) -> Result<()> {
        self.lock()?.execute(
            "UPDATE move_items SET status=?,message=? WHERE operation_id=? AND ordinal=?",
            params![status, message, id, ordinal as i64],
        )?;
        Ok(())
    }
    pub fn execute_move(&self, id: &str) -> Result<MovePlan> {
        let _operation = self
            .inner
            .file_operations
            .lock()
            .map_err(|_| anyhow::anyhow!("文件操作不可用，请重启应用。"))?;
        let (root, root_identity, items) = {
            let mut c = self.lock()?;
            let tx = c.transaction()?;
            let (state,expires,path,identity):(String,i64,Vec<u8>,String)=tx.query_row("SELECT state,expires,destination,destination_identity FROM move_operations WHERE id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            if state != "planned" {
                bail!("这批移动已提交，请查看记录，不要重复执行。");
            }
            if expires < now() {
                bail!("移动预览已过期，请重新选择目标文件夹。");
            }
            tx.execute(
                "UPDATE move_operations SET state='running' WHERE id=?",
                [id],
            )?;
            let items = {
                let mut s=tx.prepare("SELECT entry_id,path,identity,size,mtime,ordinal FROM move_items WHERE operation_id=? AND status='ready' ORDER BY ordinal")?;
                let items = s
                    .query_map([id], |r| {
                        Ok(Item {
                            id: r.get(0)?,
                            path: PathBuf::from(decode(&r.get::<_, Vec<u8>>(1)?)),
                            identity: r.get(2)?,
                            size: r.get(3)?,
                            time: r.get(4)?,
                            ordinal: r.get(5)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                items
            };
            tx.commit()?;
            (PathBuf::from(decode(&path)), identity, items)
        };
        let pause = self.inner.scan_gate.pause();
        // A target directory may already have been visited by an in-flight scan.
        // Invalidate its snapshot before moving, so its stale cleanup cannot delete
        // the new membership and recreate the moved file with a different ID.
        {
            let c = self.lock()?;
            c.execute("UPDATE scopes SET config_version=config_version+1", [])?;
            db::bump(&c)?;
        }
        for item in items {
            let cancelled: bool = self.lock()?.query_row(
                "SELECT cancel FROM move_operations WHERE id=?",
                [id],
                |r| r.get(0),
            )?;
            if cancelled {
                self.lock()?.execute("UPDATE move_items SET status='cancelled',message='已停止后续移动。' WHERE operation_id=? AND status='ready'",[id])?;
                break;
            }
            let check = (|| -> Result<PathBuf> {
                let current_root = checked_root(&root)?;
                if current_root != root || identity(&root, &fs::metadata(&root)?)? != root_identity
                {
                    bail!("目标文件夹身份发生变化，请重新选择。");
                }
                let (path, identity, size, time) = self.checked_entry(&item.id)?;
                if path != item.path
                    || identity != item.identity
                    || size != item.size
                    || time != item.time
                {
                    bail!("文件在预览后发生变化，已跳过。");
                }
                target_path(&path, &root)
            })();
            let target = match check {
                Ok(path) => path,
                Err(e) => {
                    self.record_move(id, item.ordinal, "failed", &format!("{e:#}"))?;
                    continue;
                }
            };
            self.record_move(id, item.ordinal, "running", "正在移动文件。")?;
            match move_file(&item.path, &target) {
                Ok(()) => {
                    let result = (|| -> Result<()> {
                        if !fs::symlink_metadata(&item.path)
                            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                        {
                            bail!("原位置仍有文件，请核对两个位置。");
                        }
                        let meta = fs::symlink_metadata(&target)?;
                        if !meta.is_file() || meta.len() != item.size {
                            bail!("目标文件状态与预览不符，请核对两个位置。");
                        }
                        self.reindex_move(&item.id, &target, &meta)?;
                        self.record_move(
                            id,
                            item.ordinal,
                            "succeeded",
                            &format!("已移动至 {}", display_path(&target)),
                        )
                    })();
                    if let Err(e) = result {
                        self.record_move(
                            id,
                            item.ordinal,
                            "unknown",
                            &format!("文件移动后需要核对：{e:#}。请刷新并检查目标目录。"),
                        )?;
                    }
                }
                Err(e) => {
                    let intact = fs::symlink_metadata(&item.path)
                        .ok()
                        .and_then(|m| identity(&item.path, &m).ok())
                        .as_deref()
                        == Some(&item.identity);
                    self.record_move(
                        id,
                        item.ordinal,
                        if intact { "failed" } else { "unknown" },
                        &format!("{e:#}"),
                    )?;
                }
            }
        }
        self.lock()?.execute("UPDATE move_operations SET state=CASE WHEN cancel=1 THEN 'cancelled' ELSE 'finished' END WHERE id=?",[id])?;
        drop(pause);
        // Refresh every configured folder, including previously unindexed destinations within it.
        for scope in self.summary()?.scopes {
            let _ = self.refresh(&scope.id);
        }
        self.move_plan(id)
    }
    fn reindex_move(&self, id: &str, target: &Path, meta: &fs::Metadata) -> Result<()> {
        let file_identity = identity(target, meta)?;
        let roots = {
            let c = self.lock()?;
            let mut s = c.prepare("SELECT id FROM scopes")?;
            let ids = s
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter()
                .map(|id| Ok((id.clone(), db::root(&c, &id)?)))
                .collect::<Result<Vec<_>>>()?
        };
        let mut allowed = Vec::new();
        for (scope, (root, expected, version, input)) in roots {
            if let Ok(relative) = target.strip_prefix(&root) {
                if (input.recursive || relative.components().count() == 1)
                    && !Excludes::new(&input.excludes)?.matches(relative)
                    && fs::metadata(&root)
                        .ok()
                        .and_then(|m| identity(&root, &m).ok())
                        .as_deref()
                        == Some(&expected)
                {
                    allowed.push((scope, version));
                }
            }
        }
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        tx.execute("DELETE FROM memberships WHERE entry_id=?", [id])?;
        for (scope, version) in allowed {
            tx.execute("INSERT INTO memberships(scope_id,entry_id,generation) SELECT id,?,'moved' FROM scopes WHERE id=? AND config_version=?",params![id,scope,version])?;
        }
        let visible: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memberships WHERE entry_id=?)",
            [id],
            |r| r.get(0),
        )?;
        if visible {
            let parent = target.parent().context("目标缺少父目录")?;
            let name = target.file_name().context("目标文件名无效")?;
            tx.execute(
                "INSERT OR IGNORE INTO directories(path,display) VALUES(?,?)",
                params![encode(parent.as_os_str()), display_path(parent)],
            )?;
            let dir: i64 = tx.query_row(
                "SELECT id FROM directories WHERE path=?",
                [encode(parent.as_os_str())],
                |r| r.get(0),
            )?;
            let old: Option<i64> = tx
                .query_row(
                    "SELECT id FROM entries WHERE dir_id=? AND native_name=? AND id<>?",
                    params![dir, encode(name), id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(old) = old {
                tx.execute("DELETE FROM entries WHERE id=?", [old])?;
            }
            tx.execute("UPDATE entries SET dir_id=?,native_name=?,identity=?,size=?,mtime=?,attributes=? WHERE id=?",params![dir,encode(name),file_identity,meta.len() as i64,mtime(meta),attributes(meta),id])?;
        } else {
            tx.execute("DELETE FROM entries WHERE id=?", [id])?;
        }
        db::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
}

fn move_file(source: &Path, target: &Path) -> Result<()> {
    match rename_exclusive(source, target) {
        Ok(()) => Ok(()),
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => copy_between_volumes(source, target),
        Err(e) => Err(e).context("移动失败，目标可能已有同名文件、空间不足或权限不足"),
    }
}
#[cfg(windows)]
fn rename_exclusive(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_COPY_ALLOWED, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_COPY_ALLOWED | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(unix)]
fn rename_exclusive(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(source.as_os_str().as_bytes())?;
    let to = CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(not(target_os = "macos"))]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(unix)]
fn copy_between_volumes(source: &Path, target: &Path) -> Result<()> {
    let mut input = fs::File::open(source)?;
    let original = input.metadata()?;
    let expected = file_identity(&input)?;
    let temp = target
        .parent()
        .context("目标文件夹无效")?
        .join(format!(".filem-moving-{}", uuid::Uuid::new_v4()));
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _temp = Temp(temp.clone());
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        hasher.update(&buffer[..n]);
    }
    output.sync_all()?;
    let mut verify = fs::File::open(&temp)?;
    let mut copied = Sha256::new();
    loop {
        let n = verify.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        copied.update(&buffer[..n]);
    }
    if hasher.finalize() != copied.finalize() {
        bail!("复制校验失败，原文件保持原位。");
    }
    let current = fs::symlink_metadata(source)?;
    if identity(source, &current)? != expected
        || current.len() != original.len()
        || mtime(&current) != mtime(&original)
    {
        bail!("复制期间原文件发生变化，已停止移动。");
    }
    output.set_permissions(original.permissions())?;
    if let Ok(modified) = original.modified() {
        output.set_times(fs::FileTimes::new().set_modified(modified))?;
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // Finder tags and resource forks live in extended attributes. Preserve
        // them before removing the source, and fail closed if copying fails.
        if unsafe {
            libc::fcopyfile(
                input.as_raw_fd(),
                output.as_raw_fd(),
                std::ptr::null_mut(),
                libc::COPYFILE_ACL | libc::COPYFILE_XATTR,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .context("无法保留文件扩展属性，原文件保持原位");
        }
    }
    output.sync_all()?;
    drop(output);
    rename_exclusive(&temp, target).context("目标出现同名文件，已停止移动")?;
    fs::remove_file(source).context("文件已复制到目标，但原文件未能移除，请核对两个位置")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        thread,
        time::{Duration, Instant},
    };
    fn input(path: &Path) -> ScopeInput {
        ScopeInput {
            content_enabled: false,
            path: display_path(path),
            recursive: true,
            watch: false,
            excludes: vec![],
        }
    }
    fn wait(e: &Engine) {
        let start = Instant::now();
        while e
            .summary()
            .unwrap()
            .scopes
            .iter()
            .any(|s| s.freshness != "current")
        {
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn fixture() -> (tempfile::TempDir, Engine, PathBuf, PathBuf, Vec<String>) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(source.join("a.txt"), "alpha").unwrap();
        fs::write(source.join("b.txt"), "beta").unwrap();
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        e.add_scope(input(&source)).unwrap();
        wait(&e);
        let ids = e
            .query(&Query::default())
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.id)
            .collect();
        (temp, e, source, target, ids)
    }
    #[test]
    fn multi_move_preserves_data_updates_index_and_is_single_use() {
        let (_temp, e, source, target, ids) = fixture();
        e.add_scope(input(&target)).unwrap();
        wait(&e);
        let plan = e.preview_move(&ids, &display_path(&target)).unwrap();
        assert!(source.join("a.txt").exists());
        let result = e.execute_move(&plan.operation.id).unwrap();
        assert!(result
            .operation
            .items
            .iter()
            .all(|i| i.status == "succeeded"));
        wait(&e);
        assert_eq!(fs::read_to_string(target.join("a.txt")).unwrap(), "alpha");
        assert_eq!(fs::read_to_string(target.join("b.txt")).unwrap(), "beta");
        assert!(!source.join("a.txt").exists());
        let entries = e.query(&Query::default()).unwrap().entries;
        assert_eq!(
            entries.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            ids
        );
        assert!(entries.iter().all(|i| i.path.contains("target")));
        assert_eq!(e.preview_text(&ids[0]).unwrap().text, "alpha");
        assert!(e.execute_move(&plan.operation.id).is_err());
        assert!(!e.summary().unwrap().scan_paused);
    }
    #[test]
    fn move_outside_monitored_folders_removes_only_moved_entry() {
        let (_temp, e, source, target, ids) = fixture();
        let plan = e.preview_move(&ids[..1], &display_path(&target)).unwrap();
        let result = e.execute_move(&plan.operation.id).unwrap();
        assert_eq!(result.operation.items[0].status, "succeeded");
        wait(&e);
        assert_eq!(e.summary().unwrap().total, 1);
        assert!(source.join("b.txt").exists());
        assert!(target.join("a.txt").exists());
    }
    #[test]
    fn conflicts_before_and_after_preview_never_overwrite() {
        let (_temp, e, source, target, ids) = fixture();
        fs::write(target.join("a.txt"), "keep a").unwrap();
        let plan = e.preview_move(&ids, &display_path(&target)).unwrap();
        assert_eq!(plan.operation.items[0].status, "blocked");
        fs::write(target.join("b.txt"), "keep b").unwrap();
        let result = e.execute_move(&plan.operation.id).unwrap();
        assert_eq!(result.operation.items[1].status, "failed");
        assert_eq!(fs::read_to_string(target.join("a.txt")).unwrap(), "keep a");
        assert_eq!(fs::read_to_string(target.join("b.txt")).unwrap(), "keep b");
        assert!(source.join("a.txt").exists() && source.join("b.txt").exists());
    }
    #[test]
    fn changed_target_and_source_invalidate_preview() {
        let (temp, e, source, target, ids) = fixture();
        let plan = e.preview_move(&ids, &display_path(&target)).unwrap();
        fs::rename(&target, temp.path().join("old-target")).unwrap();
        fs::create_dir(&target).unwrap();
        let result = e.execute_move(&plan.operation.id).unwrap();
        assert!(result.operation.items.iter().all(|i| i.status == "failed"));
        let plan = e.preview_move(&ids, &display_path(&target)).unwrap();
        fs::write(source.join("a.txt"), "changed after preview").unwrap();
        let result = e.execute_move(&plan.operation.id).unwrap();
        assert_eq!(result.operation.items[0].status, "failed");
        assert_eq!(result.operation.items[1].status, "succeeded");
        assert!(source.join("a.txt").exists());
    }
    #[test]
    fn duplicate_names_and_same_folder_are_skipped() {
        let (_temp, e, source, target, ids) = fixture();
        assert!(e
            .preview_move(&ids, &display_path(&source))
            .unwrap()
            .operation
            .items
            .iter()
            .all(|i| i.status == "blocked"));
        fs::create_dir(source.join("sub")).unwrap();
        fs::write(source.join("sub/a.txt"), "other a").unwrap();
        for scope in e.summary().unwrap().scopes {
            e.refresh(&scope.id).unwrap();
        }
        wait(&e);
        let ids = e
            .query(&Query::default())
            .unwrap()
            .entries
            .into_iter()
            .filter(|i| i.name == "a.txt")
            .map(|i| i.id)
            .collect::<Vec<_>>();
        let plan = e.preview_move(&ids, &display_path(&target)).unwrap();
        assert_eq!(
            plan.operation
                .items
                .iter()
                .filter(|i| i.status == "ready")
                .count(),
            1
        );
        assert_eq!(
            plan.operation
                .items
                .iter()
                .filter(|i| i.status == "blocked")
                .count(),
            1
        );
    }
    #[cfg(unix)]
    #[test]
    fn cross_volume_copy_path_verifies_contents_and_refuses_existing_target() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("中文源文件.txt");
        let target = temp.path().join("副本.txt");
        let data = vec![0xa5; 2 * 1024 * 1024 + 37];
        fs::write(&source, &data).unwrap();
        #[cfg(target_os = "macos")]
        {
            use std::{ffi::CString, os::unix::ffi::OsStrExt};
            let name = CString::new("com.filem.test").unwrap();
            let path = CString::new(source.as_os_str().as_bytes()).unwrap();
            assert_eq!(
                unsafe {
                    libc::setxattr(
                        path.as_ptr(),
                        name.as_ptr(),
                        b"preserved".as_ptr().cast(),
                        9,
                        0,
                        0,
                    )
                },
                0
            );
        }
        copy_between_volumes(&source, &target).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(&target).unwrap(), data);
        #[cfg(target_os = "macos")]
        {
            use std::{ffi::CString, os::unix::ffi::OsStrExt};
            let name = CString::new("com.filem.test").unwrap();
            let path = CString::new(target.as_os_str().as_bytes()).unwrap();
            let mut value = [0u8; 9];
            assert_eq!(
                unsafe {
                    libc::getxattr(
                        path.as_ptr(),
                        name.as_ptr(),
                        value.as_mut_ptr().cast(),
                        value.len(),
                        0,
                        0,
                    )
                },
                9
            );
            assert_eq!(&value, b"preserved");
        }
        fs::write(&source, b"keep original").unwrap();
        assert!(copy_between_volumes(&source, &target).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"keep original");
        assert_eq!(fs::read(&target).unwrap(), data);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }
    #[test]
    fn interrupted_move_keeps_journal_and_does_not_retry() {
        let (temp, e, source, target, ids) = fixture();
        let plan = e.preview_move(&ids, &display_path(&target)).unwrap();
        {
            let c = e.lock().unwrap();
            c.execute(
                "UPDATE move_operations SET state='running' WHERE id=?",
                [&plan.operation.id],
            )
            .unwrap();
            c.execute(
                "UPDATE move_items SET status='running' WHERE operation_id=? AND ordinal=0",
                [&plan.operation.id],
            )
            .unwrap();
        }
        drop(e);
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let recovered = e.move_plan(&plan.operation.id).unwrap();
        assert_eq!(recovered.operation.state, "interrupted");
        assert_eq!(recovered.operation.items[0].status, "unknown");
        assert_eq!(recovered.operation.items[1].status, "cancelled");
        assert!(e.execute_move(&plan.operation.id).is_err());
        assert!(source.join("a.txt").exists() && source.join("b.txt").exists());
    }
}
