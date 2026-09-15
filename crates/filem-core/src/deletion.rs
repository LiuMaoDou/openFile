use crate::{db, model::*, platform::*, Engine};
#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::{bail, Result};
use rusqlite::params;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

struct Snapshot {
    id: String,
    path: PathBuf,
    identity: String,
    size: u64,
    mtime: i64,
    ordinal: usize,
}

impl Engine {
    pub fn preview_delete(&self, ids: &[String]) -> Result<DeletePlan> {
        if ids.is_empty() || ids.len() > 1000 {
            bail!("请选择 1 到 1000 个文件。");
        }
        let id = uuid::Uuid::new_v4().to_string();
        let created = now();
        let mut seen = HashSet::new();
        let mut snapshots = Vec::new();
        for entry in ids.iter().filter(|entry| seen.insert((*entry).clone())) {
            let raw = {
                let c = self.lock()?;
                db::entry_path(&c, entry)
            };
            let (path, identity, size, mtime) =
                raw.map(|(p, i, s, m, _)| (p, i, s, m)).unwrap_or_default();
            let issue = self
                .checked_entry(entry)
                .and_then(|_| supported(&path))
                .err()
                .map(|e| e.to_string());
            snapshots.push((entry.clone(), path, identity, size, mtime, issue));
        }
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM operations WHERE state='planned' AND expires<?",
            [created],
        )?;
        tx.execute(
            "INSERT INTO operations(id,created,expires,state) VALUES(?,?,?,'planned')",
            params![id, created, created + 10 * 60 * 1000],
        )?;
        for (ordinal, (entry, path, identity, size, mtime, issue)) in
            snapshots.into_iter().enumerate()
        {
            let name = path
                .file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("已失效文件（{entry}）"));
            tx.execute("INSERT INTO operation_items(operation_id,ordinal,entry_id,name,path,native_path,identity,size,mtime,status,message) VALUES(?,?,?,?,?,?,?,?,?,?,?)",params![id,ordinal as i64,entry,name,path.to_string_lossy(),encode(path.as_os_str()),identity,size as i64,mtime,if issue.is_some(){"blocked"}else{"ready"},issue])?;
        }
        tx.commit()?;
        drop(c);
        self.delete_plan(&id)
    }

    pub fn delete_plan(&self, id: &str) -> Result<DeletePlan> {
        let c = self.lock()?;
        let (created, expires, state) = c.query_row(
            "SELECT created,expires,state FROM operations WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mut s = c.prepare("SELECT entry_id,name,path,size,status,message FROM operation_items WHERE operation_id=? ORDER BY ordinal")?;
        let items = s
            .query_map([id], |r| {
                Ok(DeleteItem {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: display_path_text(&r.get::<_, String>(2)?),
                    size: r.get(3)?,
                    status: r.get(4)?,
                    message: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(DeletePlan {
            id: id.into(),
            created,
            expires,
            state,
            items,
        })
    }

    pub fn delete_history(&self) -> Result<Vec<DeletePlan>> {
        let ids: Vec<String> = {
            let c = self.lock()?;
            let mut s = c.prepare(
                "SELECT id FROM operations WHERE state<>'planned' ORDER BY created DESC LIMIT 10",
            )?;
            let rows = s
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        ids.iter().map(|id| self.delete_plan(id)).collect()
    }
    pub fn cancel_delete(&self, id: &str) -> Result<()> {
        self.lock()?.execute(
            "UPDATE operations SET cancel=1 WHERE id=? AND state='running'",
            [id],
        )?;
        Ok(())
    }
    pub fn execute_delete(&self, id: &str) -> Result<DeletePlan> {
        self.execute_delete_with(id, recycle)
    }

    fn execute_delete_with(
        &self,
        id: &str,
        mut send_to_trash: impl FnMut(&Path) -> Result<()>,
    ) -> Result<DeletePlan> {
        let _guard = self
            .inner
            .file_operations
            .lock()
            .map_err(|_| anyhow::anyhow!("文件操作状态不可用，请重启应用。"))?;
        let snapshots: Vec<Snapshot> = {
            let mut c = self.lock()?;
            let tx = c.transaction()?;
            let (state, expires): (String, i64) = tx.query_row(
                "SELECT state,expires FROM operations WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if state != "planned" {
                bail!("这批删除已提交，请核对当前操作结果，不要重复执行。");
            }
            if expires < now() {
                bail!("删除预览已过期，请重新选择文件并预览。");
            }
            tx.execute("UPDATE operations SET state='running' WHERE id=?", [id])?;
            let snapshots = {
                let mut s=tx.prepare("SELECT entry_id,native_path,identity,size,mtime,ordinal FROM operation_items WHERE operation_id=? AND status='ready' ORDER BY ordinal")?;
                let rows = s
                    .query_map([id], |r| {
                        Ok(Snapshot {
                            id: r.get(0)?,
                            path: PathBuf::from(decode(&r.get::<_, Vec<u8>>(1)?)),
                            identity: r.get(2)?,
                            size: r.get(3)?,
                            mtime: r.get(4)?,
                            ordinal: r.get::<_, i64>(5)? as usize,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                rows
            };
            tx.commit()?;
            snapshots
        };
        let _scan_pause = self.inner.scan_gate.pause();
        for item in snapshots {
            let cancelled: bool =
                self.lock()?
                    .query_row("SELECT cancel FROM operations WHERE id=?", [id], |r| {
                        r.get(0)
                    })?;
            if cancelled {
                self.lock()?.execute("UPDATE operation_items SET status='cancelled',message='已停止后续删除。' WHERE operation_id=? AND status='ready'",[id])?;
                break;
            }
            let check = self
                .checked_entry(&item.id)
                .and_then(|(path, identity, size, mtime)| {
                    supported(&path)?;
                    if path != item.path
                        || identity != item.identity
                        || size != item.size
                        || mtime != item.mtime
                    {
                        bail!("文件在预览后发生变化，已跳过；请重新预览。");
                    }
                    Ok(())
                });
            if let Err(error) = check {
                self.record_delete_item(id, item.ordinal, "failed", Some(&format!("{error:#}")))?;
                continue;
            }
            // Persist intent before the OS call. An interrupted item is never retried automatically.
            self.record_delete_item(id, item.ordinal, "running", None)?;
            let result = send_to_trash(&item.path);
            let source = fs::symlink_metadata(&item.path);
            let source_identity = source
                .as_ref()
                .ok()
                .and_then(|m| identity(&item.path, m).ok());
            let same = source_identity.as_deref() == Some(item.identity.as_str());
            let confirmed_absent = source
                .as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                || source_identity
                    .as_ref()
                    .is_some_and(|current| current != &item.identity);
            match result {
                Ok(()) if confirmed_absent => {
                    let mut c = self.lock()?;
                    let tx = c.transaction()?;
                    tx.execute(
                        "DELETE FROM entries WHERE id=? AND identity=?",
                        params![item.id, item.identity],
                    )?;
                    tx.execute("UPDATE operation_items SET status='succeeded',message='系统已接收回收站操作。' WHERE operation_id=? AND ordinal=?",params![id,item.ordinal as i64])?;
                    db::bump(&tx)?;
                    tx.commit()?;
                }
                Ok(()) => self.record_delete_item(
                    id,
                    item.ordinal,
                    "unknown",
                    Some("系统返回成功，但无法确认原文件已离开源路径；请检查源路径与回收站。"),
                )?,
                Err(error) => self.record_delete_item(
                    id,
                    item.ordinal,
                    if same { "failed" } else { "unknown" },
                    Some(&format!(
                        "回收站操作失败：{error:#}。请处理错误原因后重新选择文件并删除；结果不明时先核对源路径与回收站。"
                    )),
                )?,
            }
        }
        self.lock()?.execute("UPDATE operations SET state=CASE WHEN cancel=1 THEN 'cancelled' ELSE 'finished' END WHERE id=?",[id])?;
        self.delete_plan(id)
    }
    fn record_delete_item(
        &self,
        id: &str,
        ordinal: usize,
        status: &str,
        message: Option<&str>,
    ) -> Result<()> {
        self.lock()?.execute(
            "UPDATE operation_items SET status=?,message=? WHERE operation_id=? AND ordinal=?",
            params![status, message, id, ordinal as i64],
        )?;
        Ok(())
    }
}

fn supported(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.is_file() {
        bail!("首版只删除普通文件，不删除目录。");
    }
    #[cfg(target_os = "macos")]
    if path.to_str().is_none() {
        bail!("此文件名包含非 UTF-8 字节，当前删除功能不支持；文件保持原位。");
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    bail!("当前删除功能只支持 macOS 和 Windows。");
    Ok(())
}
#[cfg(target_os = "macos")]
fn recycle(path: &Path) -> Result<()> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut context = trash::TrashContext::new();
    context.set_delete_method(DeleteMethod::NsFileManager);
    context.delete(path).context("系统废纸篓操作失败")
}
#[cfg(windows)]
fn recycle(path: &Path) -> Result<()> {
    crate::windows_shell::recycle(path)
}
#[cfg(not(any(target_os = "macos", windows)))]
fn recycle(_path: &Path) -> Result<()> {
    bail!("当前系统尚未接入回收站。");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        thread,
        time::{Duration, Instant},
    };
    use tempfile::TempDir;

    fn input(path: &Path) -> ScopeInput {
        ScopeInput {
            content_enabled: false,
            path: path.to_string_lossy().into(),
            recursive: true,
            watch: false,
            excludes: vec![],
        }
    }
    fn wait(e: &Engine, id: &str) {
        let start = Instant::now();
        while e
            .summary()
            .unwrap()
            .scopes
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .freshness
            != "current"
        {
            assert!(start.elapsed() < Duration::from_secs(15));
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn fixture() -> (TempDir, Engine, PathBuf, String, Vec<String>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), "alpha").unwrap();
        fs::write(root.join("b.txt"), "beta").unwrap();
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root)).unwrap();
        wait(&e, &scope);
        let ids = e
            .query(&Query::default())
            .unwrap()
            .entries
            .into_iter()
            .map(|item| item.id)
            .collect();
        (temp, e, root, scope, ids)
    }

    #[test]
    fn deletion_pauses_refresh_keeps_other_file_usable_and_resumes_without_resurrection() {
        let (temp, e, root, scope, ids) = fixture();
        let plan = e.preview_delete(&ids[..1]).unwrap();
        let result = e
            .execute_delete_with(&plan.id, |path| {
                assert!(e.summary()?.scan_paused);
                fs::write(root.join("during.txt"), "new while deleting")?;
                e.refresh(&scope)?;
                // Give a requested scan the chance to run; it must not see stale/pre-delete data.
                thread::sleep(Duration::from_millis(100));
                assert_eq!(e.query(&Query::default())?.total, 2);
                assert_eq!(e.preview_text(&ids[1])?.text, "beta");
                fs::rename(path, temp.path().join("recycled-a.txt"))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(result.items[0].status, "succeeded");
        assert!(!e.summary().unwrap().scan_paused);
        wait(&e, &scope);
        let entries = e.query(&Query::default()).unwrap().entries;
        assert_eq!(
            entries
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b.txt", "during.txt"]
        );
        assert_eq!(entries[0].id, ids[1]);
        assert!(e.checked_entry(&ids[0]).is_err());
        assert_eq!(e.preview_text(&ids[1]).unwrap().text, "beta");
    }

    #[test]
    fn preview_is_read_only_deduplicated_and_single_use() {
        let (temp, e, root, _, ids) = fixture();
        let parent = e.add_scope(input(temp.path())).unwrap();
        wait(&e, &parent);
        assert_eq!(e.summary().unwrap().scopes.len(), 2);
        let plan = e.preview_delete(&[ids[0].clone(), ids[0].clone()]).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].status, "ready");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "alpha");
        assert!(e.delete_history().unwrap().is_empty());
        let trash = temp.path().join("test-trash");
        fs::create_dir(&trash).unwrap();
        let result = e
            .execute_delete_with(&plan.id, |path| {
                fs::rename(path, trash.join(path.file_name().unwrap()))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(result.items[0].status, "succeeded");
        assert!(!root.join("a.txt").exists());
        assert_eq!(fs::read_to_string(trash.join("a.txt")).unwrap(), "alpha");
        assert_eq!(e.summary().unwrap().total, 1);
        assert!(e
            .execute_delete_with(&plan.id, |_| panic!("must not execute twice"))
            .is_err());
        assert_eq!(e.delete_history().unwrap()[0].id, plan.id);
    }

    #[test]
    fn modified_reindexed_file_does_not_inherit_preview_authorization() {
        let (_temp, e, root, scope, ids) = fixture();
        let plan = e.preview_delete(&ids[..1]).unwrap();
        fs::write(root.join("a.txt"), "changed content").unwrap();
        e.refresh(&scope).unwrap();
        wait(&e, &scope);
        let result = e
            .execute_delete_with(&plan.id, |_| panic!("changed file must be skipped"))
            .unwrap();
        assert_eq!(result.items[0].status, "failed");
        assert_eq!(
            fs::read_to_string(root.join("a.txt")).unwrap(),
            "changed content"
        );
    }

    #[test]
    fn replaced_file_and_removed_scope_are_rejected() {
        let (_temp, e, root, scope, ids) = fixture();
        let replaced = e.preview_delete(&ids[..1]).unwrap();
        let revoked = e.preview_delete(&ids[1..]).unwrap();
        fs::rename(root.join("a.txt"), root.join("original.txt")).unwrap();
        fs::write(root.join("a.txt"), "alpha").unwrap();
        let result = e
            .execute_delete_with(&replaced.id, |_| panic!("replaced identity"))
            .unwrap();
        assert_eq!(result.items[0].status, "failed");
        e.remove_scope(&scope).unwrap();
        let result = e
            .execute_delete_with(&revoked.id, |_| panic!("scope was revoked"))
            .unwrap();
        assert!(result.items.iter().all(|item| item.status == "failed"));
        assert!(root.join("a.txt").exists() && root.join("b.txt").exists());
    }

    #[test]
    fn partial_failure_preserves_file_and_continues_batch() {
        let (temp, e, root, _, ids) = fixture();
        let plan = e.preview_delete(&ids).unwrap();
        let result = e
            .execute_delete_with(&plan.id, |path| {
                if path.ends_with("a.txt") {
                    bail!("simulated unavailable recycle bin");
                }
                fs::rename(path, temp.path().join("recycled-b.txt"))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(result.items[0].status, "failed");
        assert_eq!(result.items[1].status, "succeeded");
        assert!(root.join("a.txt").exists());
        assert!(!root.join("b.txt").exists());
        assert_eq!(e.summary().unwrap().total, 1);
    }

    #[test]
    fn stop_finishes_inflight_file_then_skips_remaining() {
        let (temp, e, root, _, ids) = fixture();
        let plan = e.preview_delete(&ids).unwrap();
        let result = e
            .execute_delete_with(&plan.id, |path| {
                fs::rename(path, temp.path().join("recycled-a.txt"))?;
                e.cancel_delete(&plan.id)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(result.state, "cancelled");
        assert_eq!(result.items[0].status, "succeeded");
        assert_eq!(result.items[1].status, "cancelled");
        assert!(root.join("b.txt").exists());
    }

    #[test]
    fn success_without_moving_file_is_not_reported_as_deleted() {
        let (_temp, e, root, _, ids) = fixture();
        let plan = e.preview_delete(&ids[..1]).unwrap();
        let result = e.execute_delete_with(&plan.id, |_| Ok(())).unwrap();
        assert_eq!(result.items[0].status, "unknown");
        assert!(root.join("a.txt").exists());
        assert_eq!(e.summary().unwrap().total, 2);
    }

    #[test]
    fn expired_plan_cannot_execute() {
        let (_temp, e, root, _, ids) = fixture();
        let plan = e.preview_delete(&ids).unwrap();
        e.lock()
            .unwrap()
            .execute("UPDATE operations SET expires=0 WHERE id=?", [&plan.id])
            .unwrap();
        assert!(e
            .execute_delete_with(&plan.id, |_| panic!("expired plan"))
            .is_err());
        assert!(root.join("a.txt").exists());
    }

    #[test]
    fn interrupted_journal_recovers_without_retry() {
        let (temp, e, root, _, ids) = fixture();
        let plan = e.preview_delete(&ids).unwrap();
        e.lock()
            .unwrap()
            .execute(
                "UPDATE operations SET state='running' WHERE id=?",
                [&plan.id],
            )
            .unwrap();
        e.record_delete_item(&plan.id, 0, "running", None).unwrap();
        drop(e);
        let reopened = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let result = reopened.delete_plan(&plan.id).unwrap();
        assert_eq!(result.state, "interrupted");
        assert_eq!(result.items[0].status, "unknown");
        assert_eq!(result.items[1].status, "cancelled");
        assert!(reopened
            .execute_delete_with(&plan.id, |_| panic!("no automatic retry"))
            .is_err());
        assert!(root.join("a.txt").exists() && root.join("b.txt").exists());
    }
}
