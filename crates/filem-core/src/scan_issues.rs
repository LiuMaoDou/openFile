//! Last unresolved scan failures, kept independently from file memberships.
use crate::{db, model::*, platform::*, Engine};
use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct Failure {
    pub path: PathBuf,
    pub kind: &'static str,
    pub reason: String,
}
impl Failure {
    pub fn new(path: &Path, kind: &'static str, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind,
            reason: reason.into(),
        }
    }
    pub fn io(path: &Path, kind: &'static str, action: &str, error: &std::io::Error) -> Self {
        let reason = match error.kind() {
            std::io::ErrorKind::PermissionDenied => "没有访问权限",
            std::io::ErrorKind::NotFound => "路径不存在或已被移动",
            std::io::ErrorKind::NotADirectory => "路径已不再是文件夹",
            _ => "系统无法完成读取",
        };
        Self::new(path, kind, format!("{action}：{reason}（{error}）"))
    }
}

pub(crate) fn migrate(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS scan_issues (
        scope_id TEXT NOT NULL REFERENCES scopes(id) ON DELETE CASCADE,
        path BLOB NOT NULL, kind TEXT NOT NULL, reason TEXT NOT NULL,
        PRIMARY KEY(scope_id,path));",
    )?;
    Ok(())
}
pub(crate) fn count(c: &Connection, id: &str) -> Result<u64> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM scan_issues WHERE scope_id=?",
        [id],
        |r| r.get(0),
    )?)
}
pub(crate) fn summary(count: u64) -> String {
    format!("{count} 个路径无法完整扫描，保留其旧索引。")
}
pub(crate) fn is_summary(message: &str) -> bool {
    message
        .strip_suffix(" 个路径无法完整扫描，保留其旧索引。")
        .is_some_and(|prefix| prefix.parse::<u64>().is_ok())
}

/// A subtree rescan only replaces issues inside that subtree; other failures stay.
pub(crate) fn replace(
    c: &Connection,
    id: &str,
    limits: Option<&[PathBuf]>,
    failures: &[Failure],
) -> Result<()> {
    if let Some(limits) = limits {
        let paths = c
            .prepare("SELECT path FROM scan_issues WHERE scope_id=?")?
            .query_map([id], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for bytes in paths {
            let path = PathBuf::from(decode(&bytes));
            if limits.iter().any(|root| path.starts_with(root)) {
                c.execute(
                    "DELETE FROM scan_issues WHERE scope_id=? AND path=?",
                    params![id, bytes],
                )?;
            }
        }
    } else {
        c.execute("DELETE FROM scan_issues WHERE scope_id=?", [id])?;
    }
    let mut insert = c.prepare_cached(
        "INSERT INTO scan_issues(scope_id,path,kind,reason) VALUES(?,?,?,?)
        ON CONFLICT(scope_id,path) DO UPDATE SET kind=excluded.kind,reason=excluded.reason",
    )?;
    for failure in failures {
        insert.execute(params![
            id,
            encode(failure.path.as_os_str()),
            failure.kind,
            failure.reason
        ])?;
    }
    Ok(())
}

pub(crate) fn root_failure(
    engine: &Engine,
    id: &str,
    availability: &str,
    failure: Failure,
) -> Result<()> {
    let mut c = engine.lock()?;
    let tx = c.transaction()?;
    replace(&tx, id, None, std::slice::from_ref(&failure))?;
    db::set_state(&tx, id, availability, "partial", Some(&failure.reason))?;
    tx.commit()?;
    Ok(())
}

impl Engine {
    pub fn scan_issues(&self, id: &str, offset: usize, limit: usize) -> Result<ScanIssuePage> {
        let c = self.inner.query_db.lock().unwrap();
        let tx = c.unchecked_transaction()?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM scopes WHERE id=?)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            bail!("文件夹已移除，请刷新后查看。");
        }
        let total = count(&tx, id)?;
        let limit = limit.clamp(1, 100);
        let offset = offset.min(i64::MAX as usize);
        let items = tx.prepare("SELECT path,kind,reason FROM scan_issues WHERE scope_id=? ORDER BY path LIMIT ? OFFSET ?")?
            .query_map(params![id, limit as i64, offset as i64], |r| {
                Ok(ScanIssue {
                    path: display_path(Path::new(&decode(&r.get::<_, Vec<u8>>(0)?))),
                    kind: r.get(1)?, reason: r.get(2)?,
                })
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ScanIssuePage {
            items,
            total,
            offset,
            limit,
        })
    }
}
