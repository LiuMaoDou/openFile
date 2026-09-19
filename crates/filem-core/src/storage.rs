//! A small, fixed bootstrap database selects the index before workers start.
//! Migration uses SQLite backup (including WAL), never a live file copy.
use crate::{platform::*, Engine};
use anyhow::{bail, Context, Result};
use rusqlite::{backup::Backup, params, Connection, OpenFlags};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

pub(crate) struct ManagedStorage {
    config: Mutex<Connection>,
    // Held as long as the engine and all its workers; prevents two destinations
    // becoming active under the same application profile.
    _lock: fs::File,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStorage {
    pub path: String,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub total_bytes: u64,
    pub pending_path: Option<String>,
    pub previous_path: Option<String>,
    pub migration_error: Option<String>,
    pub can_change: bool,
}

fn lock_file(path: &Path) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock()
        .map_err(|_| anyhow::anyhow!("索引正由另一个 FileM 实例使用，请先退出该实例。"))?;
    Ok(file)
}

fn stored_path(c: &Connection, column: &str) -> Result<Option<PathBuf>> {
    let bytes: Option<Vec<u8>> = c.query_row(
        &format!("SELECT {column} FROM storage WHERE id=1"),
        [],
        |r| r.get(0),
    )?;
    Ok(bytes.map(|v| PathBuf::from(decode(&v))))
}

fn unused_destination(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        bail!("请选择一个已存在的文件夹。");
    }
    for name in [
        "index.sqlite",
        "index.sqlite-wal",
        "index.sqlite-shm",
        "index.sqlite-journal",
    ] {
        if fs::symlink_metadata(dir.join(name)).is_ok() {
            bail!("目标文件夹已有索引文件，请选择其他文件夹；不会覆盖原有文件。");
        }
    }
    for entry in fs::read_dir(dir)? {
        if entry?.file_name() != ".filem-index.lock" {
            bail!("请选择专门存放索引的空文件夹。索引目录不会参与文件扫描，请勿选择资料文件夹或整个磁盘。");
        }
    }
    Ok(())
}

fn migrate(source: &Path, destination: &Path) -> Result<()> {
    let dir = destination.parent().context("目标路径没有父目录")?;
    unused_destination(dir)?;
    let _source_lock = lock_file(
        &source
            .parent()
            .context("原索引路径无效")?
            .join(".filem-index.lock"),
    )?;
    let _destination_lock = lock_file(&dir.join(".filem-index.lock"))?;
    // Recheck after acquiring the target lock. persist_noclobber also prevents
    // overwriting a file introduced by another process during the backup.
    unused_destination(dir)?;
    let source_db = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let temp = tempfile::Builder::new()
        .prefix(".filem-migration-")
        .suffix(".sqlite")
        .tempfile_in(dir)?;
    {
        let mut destination_db = Connection::open(temp.path())?;
        {
            let backup = Backup::new(&source_db, &mut destination_db)?;
            let mut busy = 0;
            loop {
                use rusqlite::backup::StepResult;
                match backup.step(256)? {
                    StepResult::Done => break,
                    StepResult::More => busy = 0,
                    StepResult::Busy | StepResult::Locked => {
                        busy += 1;
                        if busy > 50 {
                            bail!("原索引持续被占用，请退出其他使用索引的程序后重试。");
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    _ => bail!("无法完成索引迁移"),
                }
            }
        }
        // Publish a self-contained database, with no temporary WAL dependency.
        destination_db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")?;
        let check: String = destination_db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            bail!("迁移后的索引校验失败：{check}");
        }
        destination_db.close().map_err(|(_, e)| e)?;
    }
    temp.as_file().sync_all()?;
    temp.persist_noclobber(destination).map_err(|e| e.error)?;
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

impl Engine {
    /// Used by both the desktop app and local dev server. A missing relocated
    /// index is an error, never permission to silently create an empty library.
    pub fn open_managed(data_dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(data_dir.as_ref())?;
        let data_dir = data_dir.as_ref().canonicalize()?;
        let profile_lock = lock_file(&data_dir.join(".filem-storage.lock"))?;
        let c = Connection::open(data_dir.join("storage.sqlite"))?;
        c.execute_batch("PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS storage(id INTEGER PRIMARY KEY CHECK(id=1), current BLOB NOT NULL,
            pending BLOB, pending_identity TEXT, previous BLOB, error TEXT, initialized INTEGER NOT NULL DEFAULT 0);")?;
        let default_path = data_dir.join("index/index.sqlite");
        c.execute(
            "INSERT OR IGNORE INTO storage(id,current) VALUES(1,?)",
            [encode(default_path.as_os_str())],
        )?;
        let mut current = stored_path(&c, "current")?.context("索引位置配置缺失")?;
        let initialized: bool =
            c.query_row("SELECT initialized FROM storage WHERE id=1", [], |r| {
                r.get(0)
            })?;
        if initialized && !current.is_file() {
            bail!(
                "索引文件不可用：{}。请连接对应磁盘或恢复该文件后重新启动；不会创建空白索引。",
                display_path(&current)
            );
        }
        if let Some(destination) = stored_path(&c, "pending")? {
            let result = (|| -> Result<()> {
                let dir = destination.parent().context("新索引路径无效")?;
                let expected: String =
                    c.query_row("SELECT pending_identity FROM storage WHERE id=1", [], |r| {
                        r.get(0)
                    })?;
                if identity(dir, &fs::metadata(dir)?)? != expected {
                    bail!("目标文件夹身份已变化，请重新选择存放位置。");
                }
                migrate(&current, &destination)?;
                c.execute("UPDATE storage SET previous=current,current=pending,pending=NULL,pending_identity=NULL,error=NULL WHERE id=1", [])?;
                Ok(())
            })();
            match result {
                Ok(()) => current = destination,
                Err(error) => {
                    c.execute(
                        "UPDATE storage SET error=? WHERE id=1",
                        [format!("迁移未完成，仍使用原位置：{error:#}")],
                    )?;
                }
            }
        }
        let engine = Self::open_with_storage(
            &current,
            Some(ManagedStorage {
                config: Mutex::new(c),
                _lock: profile_lock,
            }),
        )?;
        engine
            .inner
            .managed_storage
            .as_ref()
            .unwrap()
            .config
            .lock()
            .unwrap()
            .execute(
                "UPDATE storage SET initialized=1 WHERE id=1 AND initialized=0",
                [],
            )?;
        Ok(engine)
    }

    pub fn index_storage(&self) -> Result<IndexStorage> {
        let size = |name: &str| -> Result<u64> {
            match fs::metadata(self.inner.storage.join(name)) {
                Ok(meta) => Ok(meta.len()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
                Err(e) => Err(e.into()),
            }
        };
        let database_bytes = size("index.sqlite")?;
        let wal_bytes = size("index.sqlite-wal")?;
        let (pending_path, previous_path, migration_error) =
            if let Some(managed) = &self.inner.managed_storage {
                let c = managed.config.lock().unwrap();
                (
                    stored_path(&c, "pending")?.map(|p| display_path(&p)),
                    stored_path(&c, "previous")?.map(|p| display_path(&p)),
                    c.query_row("SELECT error FROM storage WHERE id=1", [], |r| r.get(0))?,
                )
            } else {
                (None, None, None)
            };
        Ok(IndexStorage {
            path: display_path(&self.inner.storage.join("index.sqlite")),
            database_bytes,
            wal_bytes,
            total_bytes: database_bytes
                .saturating_add(wal_bytes)
                .saturating_add(size("index.sqlite-shm")?),
            pending_path,
            previous_path,
            migration_error,
            can_change: self.inner.managed_storage.is_some(),
        })
    }

    pub fn set_index_location(&self, path: &str) -> Result<()> {
        let managed = self
            .inner
            .managed_storage
            .as_ref()
            .context("此索引不支持修改启动位置")?;
        let path = Path::new(path);
        if !path.is_absolute() {
            bail!("请输入完整的绝对文件夹路径。");
        }
        let dir = path.canonicalize().context("目标文件夹不存在或无法访问")?;
        if dir == self.inner.storage {
            bail!("这已经是当前索引所在的文件夹。");
        }
        unused_destination(&dir)?;
        let _destination_lock = lock_file(&dir.join(".filem-index.lock"))?;
        let mut probe = tempfile::Builder::new()
            .prefix(".filem-write-check-")
            .tempfile_in(&dir)
            .context("目标文件夹不可写")?;
        probe.write_all(b"FileM")?;
        probe.as_file().sync_all()?;
        let identity = identity(&dir, &fs::metadata(&dir)?)?;
        let c = managed.config.lock().unwrap();
        c.execute(
            "UPDATE storage SET pending=?,pending_identity=?,error=NULL WHERE id=1",
            params![encode(dir.join("index.sqlite").as_os_str()), identity],
        )?;
        Ok(())
    }

    pub fn cancel_index_location(&self) -> Result<()> {
        let managed = self
            .inner
            .managed_storage
            .as_ref()
            .context("此索引不支持修改启动位置")?;
        managed.config.lock().unwrap().execute(
            "UPDATE storage SET pending=NULL,pending_identity=NULL,error=NULL WHERE id=1",
            [],
        )?;
        Ok(())
    }

    pub fn reveal_index(&self, previous: bool) -> Result<()> {
        let path = if previous {
            let managed = self
                .inner
                .managed_storage
                .as_ref()
                .context("没有旧索引位置")?;
            stored_path(&managed.config.lock().unwrap(), "previous")?.context("没有旧索引位置")?
        } else {
            self.inner.storage.join("index.sqlite")
        };
        if !path.is_file() {
            bail!("索引文件不存在或所在磁盘不可用。");
        }
        #[cfg(windows)]
        crate::windows_shell::open(&path, true)?;
        #[cfg(target_os = "macos")]
        if !std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .status()?
            .success()
        {
            bail!("系统无法定位索引文件。");
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        open::that(path.parent().context("索引路径无效")?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_default_is_adopted_and_migration_is_deferred_and_persistent() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let target = temp.path().join("新位置");
        fs::create_dir(&target).unwrap();
        let legacy = Engine::open(profile.join("index/index.sqlite")).unwrap();
        legacy
            .lock()
            .unwrap()
            .execute("INSERT INTO settings VALUES('test_setting','keep')", [])
            .unwrap();
        legacy
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO hidden_directories(id,root,path) VALUES('rule',X'01','hidden-folder')",
                [],
            )
            .unwrap();
        drop(legacy);
        let engine = Engine::open_managed(&profile).unwrap();
        let original = engine.index_storage().unwrap().path;
        engine.set_index_location(target.to_str().unwrap()).unwrap();
        assert_eq!(engine.index_storage().unwrap().path, original);
        assert!(engine.index_storage().unwrap().pending_path.is_some());
        assert!(!target.join("index.sqlite").exists());
        // Changes after clicking Save must also be present after restart.
        engine
            .lock()
            .unwrap()
            .execute(
                "UPDATE settings SET value='latest' WHERE key='test_setting'",
                [],
            )
            .unwrap();
        drop(engine);
        let engine = Engine::open_managed(&profile).unwrap();
        let info = engine.index_storage().unwrap();
        assert_eq!(
            info.path,
            display_path(&target.canonicalize().unwrap().join("index.sqlite"))
        );
        assert_eq!(info.previous_path.as_deref(), Some(original.as_str()));
        assert!(info.pending_path.is_none());
        assert!(info.migration_error.is_none());
        assert_eq!(
            engine
                .lock()
                .unwrap()
                .query_row(
                    "SELECT value FROM settings WHERE key='test_setting'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "latest"
        );
        assert_eq!(
            engine
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM hidden_directories", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(Path::new(&original).exists());
        assert!(info.total_bytes >= info.database_bytes + info.wal_bytes);
        drop(engine);
        let engine = Engine::open_managed(&profile).unwrap();
        assert_eq!(engine.index_storage().unwrap().path, info.path);
    }

    #[test]
    fn backup_includes_uncheckpointed_wal_and_fts() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        let path = source.join("index.sqlite");
        // Keep the source connection open so the WAL is not checkpointed away.
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE VIRTUAL TABLE documents USING fts5(text,tokenize='trigram');
            INSERT INTO documents(text) VALUES('中文合同 keyword latest');",
        )
        .unwrap();
        assert!(source.join("index.sqlite-wal").metadata().unwrap().len() > 0);
        migrate(&path, &target.join("index.sqlite")).unwrap();
        let copy = Connection::open(target.join("index.sqlite")).unwrap();
        assert_eq!(
            copy.query_row(
                "SELECT text FROM documents WHERE documents MATCH 'keyword'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "中文合同 keyword latest"
        );
        assert_eq!(
            copy.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(!target.join("index.sqlite-wal").exists());
    }

    #[test]
    fn cancel_and_invalid_targets_do_not_change_active_storage() {
        let temp = tempfile::tempdir().unwrap();
        let engine = Engine::open_managed(temp.path().join("profile")).unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        assert!(engine.set_index_location("relative/path").is_err());
        assert!(engine
            .set_index_location(temp.path().join("missing").to_str().unwrap())
            .is_err());
        assert!(engine
            .set_index_location(engine.inner.storage.to_str().unwrap())
            .is_err());
        engine.set_index_location(target.to_str().unwrap()).unwrap();
        engine.cancel_index_location().unwrap();
        assert!(engine.index_storage().unwrap().pending_path.is_none());
        let marker = target.join("index.sqlite");
        fs::write(&marker, b"do not overwrite").unwrap();
        assert!(engine.set_index_location(target.to_str().unwrap()).is_err());
        assert_eq!(fs::read(marker).unwrap(), b"do not overwrite");
    }

    #[test]
    fn destination_collision_after_saving_falls_back_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let engine = Engine::open_managed(&profile).unwrap();
        let original = engine.index_storage().unwrap().path;
        engine.set_index_location(target.to_str().unwrap()).unwrap();
        drop(engine);
        fs::write(target.join("index.sqlite"), b"unrelated data").unwrap();
        let engine = Engine::open_managed(&profile).unwrap();
        let info = engine.index_storage().unwrap();
        assert_eq!(info.path, original);
        assert!(info.migration_error.unwrap().contains("已有索引文件"));
        assert!(info.pending_path.is_some());
        assert_eq!(
            fs::read(target.join("index.sqlite")).unwrap(),
            b"unrelated data"
        );
    }

    #[test]
    fn missing_destination_falls_back_and_can_be_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let engine = Engine::open_managed(&profile).unwrap();
        engine.set_index_location(target.to_str().unwrap()).unwrap();
        drop(engine);
        fs::remove_dir_all(&target).unwrap();
        let engine = Engine::open_managed(&profile).unwrap();
        assert!(engine.index_storage().unwrap().migration_error.is_some());
        engine.cancel_index_location().unwrap();
        assert!(engine.index_storage().unwrap().migration_error.is_none());
    }

    #[test]
    fn missing_active_index_does_not_create_an_empty_library() {
        let temp = tempfile::tempdir().unwrap();
        let engine = Engine::open_managed(temp.path()).unwrap();
        let path = engine.inner.storage.join("index.sqlite");
        drop(engine);
        fs::remove_file(&path).unwrap();
        assert!(Engine::open_managed(temp.path()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn profile_and_destination_locks_are_respected() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let engine = Engine::open_managed(&profile).unwrap();
        assert!(Engine::open_managed(&profile).is_err());
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let _lock = lock_file(&target.join(".filem-index.lock")).unwrap();
        assert!(engine.set_index_location(target.to_str().unwrap()).is_err());
    }

    #[test]
    fn document_folders_are_rejected_before_save_and_before_migration() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let document = target.join("合同.txt");
        fs::write(&document, "keep this document").unwrap();
        let engine = Engine::open_managed(&profile).unwrap();
        let original = engine.index_storage().unwrap().path;
        assert!(engine
            .set_index_location(target.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("空文件夹"));
        fs::remove_file(&document).unwrap();
        engine.set_index_location(target.to_str().unwrap()).unwrap();
        drop(engine);
        fs::write(&document, "created after save").unwrap();
        let engine = Engine::open_managed(&profile).unwrap();
        let info = engine.index_storage().unwrap();
        assert_eq!(info.path, original);
        assert!(info.migration_error.unwrap().contains("空文件夹"));
        assert!(!target.join("index.sqlite").exists());
        assert_eq!(fs::read_to_string(document).unwrap(), "created after save");
    }
}
