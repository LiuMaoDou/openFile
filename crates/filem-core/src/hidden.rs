//! Directory rules affect the view, independently of individual file hiding.
use crate::{db, model::HiddenDirectory, platform::*, Engine};
use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub(crate) fn migrate(c: &Connection) -> Result<()> {
    let columns = c
        .prepare("PRAGMA table_info(directories)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let tx = c.unchecked_transaction()?;
    if !columns.iter().any(|column| column == "hidden") {
        tx.execute_batch("ALTER TABLE directories ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;")?;
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS hidden_directories (
            id TEXT PRIMARY KEY, root BLOB NOT NULL UNIQUE, path TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS directories_hidden ON directories(hidden);
         CREATE INDEX IF NOT EXISTS entries_directory ON entries(dir_id);
         PRAGMA user_version=5;",
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn list(c: &Connection) -> Result<Vec<HiddenDirectory>> {
    Ok(
        c.prepare("SELECT id,path FROM hidden_directories ORDER BY rowid")?
            .query_map([], |r| {
                Ok(HiddenDirectory {
                    id: r.get(0)?,
                    path: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?,
    )
}

pub(crate) fn roots(c: &Connection) -> Result<Vec<PathBuf>> {
    Ok(c.prepare("SELECT root FROM hidden_directories")?
        .query_map([], |r| Ok(PathBuf::from(decode(&r.get::<_, Vec<u8>>(0)?))))?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn contains(roots: &[PathBuf], directory: &Path) -> bool {
    roots.iter().any(|root| directory.starts_with(root))
}

fn update_directories(c: &Connection) -> Result<()> {
    let roots = roots(c)?;
    let mut directories = c.prepare("SELECT id,path,hidden FROM directories")?;
    let mut rows = directories.query([])?;
    let mut update = c.prepare("UPDATE directories SET hidden=? WHERE id=?")?;
    while let Some(row) = rows.next()? {
        let path = PathBuf::from(decode(&row.get::<_, Vec<u8>>(1)?));
        let hidden = contains(&roots, &path);
        if hidden != row.get::<_, bool>(2)? {
            update.execute(params![hidden, row.get::<_, i64>(0)?])?;
        }
    }
    Ok(())
}

impl Engine {
    pub fn hide_directory(&self, path: &Path) -> Result<()> {
        let root = checked_root(path)?;
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        let scope_roots = tx
            .prepare("SELECT root FROM scopes")?
            .query_map([], |r| Ok(PathBuf::from(decode(&r.get::<_, Vec<u8>>(0)?))))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let intersects = scope_roots
            .iter()
            .any(|scope_root| root.starts_with(scope_root) || scope_root.starts_with(&root));
        if !intersects {
            bail!("请选择已添加的监控文件夹、其子目录或包含它的上级目录。");
        }
        tx.execute(
            "INSERT OR IGNORE INTO hidden_directories(id,root,path) VALUES(?,?,?)",
            params![
                uuid::Uuid::new_v4().to_string(),
                encode(root.as_os_str()),
                display_path(&root)
            ],
        )?;
        update_directories(&tx)?;
        db::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn restore_directory(&self, id: &str) -> Result<()> {
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        if tx.execute("DELETE FROM hidden_directories WHERE id=?", [id])? == 0 {
            bail!("该目录隐藏规则已不存在，请重新打开隐藏目录。");
        }
        update_directories(&tx)?;
        db::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
}
