use crate::{model::*, platform::*};
use anyhow::{bail, Result};
use rusqlite::{params, params_from_iter, types::Value, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};

pub fn connect_reader(path: &Path) -> Result<Connection> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(c)
}

pub fn connect(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > 5 {
        bail!("索引版本高于当前应用支持的版本，请使用更新的 FileM。");
    }
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
      CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS meta (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
      INSERT OR IGNORE INTO meta VALUES(1,0);
      CREATE TABLE IF NOT EXISTS scopes (id TEXT PRIMARY KEY, root BLOB NOT NULL UNIQUE, path TEXT NOT NULL, name TEXT NOT NULL,
        identity TEXT NOT NULL, depth INTEGER NOT NULL, recursive INTEGER NOT NULL, watch INTEGER NOT NULL, excludes TEXT NOT NULL,
        availability TEXT NOT NULL DEFAULT 'available', freshness TEXT NOT NULL DEFAULT 'unscanned', scanned INTEGER NOT NULL DEFAULT 0,
        last_scan INTEGER, message TEXT, config_version INTEGER NOT NULL DEFAULT 1);
      CREATE TABLE IF NOT EXISTS directories (id INTEGER PRIMARY KEY AUTOINCREMENT, path BLOB NOT NULL UNIQUE, display TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS entries (id INTEGER PRIMARY KEY AUTOINCREMENT, dir_id INTEGER NOT NULL REFERENCES directories(id),
        native_name BLOB NOT NULL, name TEXT NOT NULL, extension TEXT NOT NULL, group_name TEXT NOT NULL, size INTEGER NOT NULL,
        mtime INTEGER NOT NULL, identity TEXT NOT NULL, attributes INTEGER NOT NULL, hidden INTEGER NOT NULL DEFAULT 0,
        UNIQUE(dir_id,native_name));
      CREATE TABLE IF NOT EXISTS memberships (scope_id TEXT NOT NULL REFERENCES scopes(id) ON DELETE CASCADE,
        entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE, generation TEXT NOT NULL,
        PRIMARY KEY(scope_id,entry_id));
      CREATE INDEX IF NOT EXISTS entries_extension ON entries(extension,hidden);
      CREATE INDEX IF NOT EXISTS entries_group ON entries(group_name,hidden);
      CREATE INDEX IF NOT EXISTS entries_name ON entries(name COLLATE NOCASE);
      CREATE INDEX IF NOT EXISTS memberships_entry ON memberships(entry_id);
      CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY,created INTEGER NOT NULL,expires INTEGER NOT NULL,state TEXT NOT NULL,cancel INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS operation_items(operation_id TEXT NOT NULL REFERENCES operations(id) ON DELETE CASCADE,ordinal INTEGER NOT NULL,entry_id TEXT NOT NULL,name TEXT NOT NULL,path TEXT NOT NULL,native_path BLOB NOT NULL,identity TEXT NOT NULL,size INTEGER NOT NULL,mtime INTEGER NOT NULL,status TEXT NOT NULL,message TEXT,PRIMARY KEY(operation_id,ordinal));
      UPDATE operation_items SET status='unknown',message='应用在操作期间退出，请检查源路径与系统回收站；不会自动重试。' WHERE status='running';
      UPDATE operation_items SET status='cancelled',message='上次操作中断，未执行此项。' WHERE status='ready' AND operation_id IN (SELECT id FROM operations WHERE state='running');
      UPDATE operations SET state='interrupted' WHERE state='running';
      CREATE TABLE IF NOT EXISTS move_operations(id TEXT PRIMARY KEY,created INTEGER NOT NULL,expires INTEGER NOT NULL,state TEXT NOT NULL,cancel INTEGER NOT NULL DEFAULT 0,destination BLOB NOT NULL,destination_identity TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS move_items(operation_id TEXT NOT NULL REFERENCES move_operations(id) ON DELETE CASCADE,ordinal INTEGER NOT NULL,entry_id TEXT NOT NULL,name TEXT NOT NULL,path BLOB NOT NULL,identity TEXT NOT NULL,size INTEGER NOT NULL,mtime INTEGER NOT NULL,status TEXT NOT NULL,message TEXT,PRIMARY KEY(operation_id,ordinal));
      UPDATE move_items SET status='unknown',message='上次移动中断，请核对原位置和目标位置；不会自动重试。' WHERE status='running';
      UPDATE move_items SET status='cancelled',message='上次操作中断，未执行此项。' WHERE status='ready' AND operation_id IN (SELECT id FROM move_operations WHERE state='running');
      UPDATE move_operations SET state='interrupted' WHERE state='running';
")?;
    crate::content::migrate(&c)?;
    crate::hidden::migrate(&c)?;
    crate::scan_issues::migrate(&c)?;
    c.execute("UPDATE scopes SET freshness='verifying', availability='offline', message='正在校验上次索引'",[])?;
    Ok(c)
}
pub fn bump(c: &Connection) -> Result<()> {
    c.execute("UPDATE meta SET revision=revision+1 WHERE id=1", [])?;
    Ok(())
}
pub fn revision(c: &Connection) -> Result<u64> {
    Ok(c.query_row("SELECT revision FROM meta WHERE id=1", [], |r| r.get(0))?)
}
pub fn scopes(c: &Connection) -> Result<Vec<Scope>> {
    let mut s=c.prepare("SELECT s.id,name,path,recursive,watch,excludes,availability,freshness,
      (SELECT COUNT(*) FROM memberships m WHERE m.scope_id=s.id),scanned,last_scan,message,content_enabled,content_paused,
      (SELECT COUNT(*) FROM scan_issues i WHERE i.scope_id=s.id) FROM scopes s ORDER BY rowid")?;
    let result = s
        .query_map([], |r| {
            Ok(Scope {
                scan_issue_count: r.get(14)?,
                progress: None,
                content_enabled: r.get(12)?,
                content_paused: r.get(13)?,
                id: r.get(0)?,
                name: display_path_text(&r.get::<_, String>(1)?),
                path: display_path_text(&r.get::<_, String>(2)?),
                recursive: r.get(3)?,
                watch: r.get(4)?,
                excludes: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
                availability: r.get(6)?,
                freshness: r.get(7)?,
                count: r.get(8)?,
                scanned: r.get(9)?,
                last_scan: r.get(10)?,
                message: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(result)
}
pub fn root(c: &Connection, id: &str) -> Result<(PathBuf, String, i64, ScopeInput)> {
    Ok(c.query_row(
        "SELECT root,identity,config_version,path,recursive,watch,excludes,content_enabled FROM scopes WHERE id=?",
        [id],
        |r| {
            Ok((
                PathBuf::from(decode(&r.get::<_, Vec<u8>>(0)?)),
                r.get(1)?,
                r.get(2)?,
                ScopeInput {
                    content_enabled: r.get(7)?,                    path: display_path_text(&r.get::<_, String>(3)?),
                    recursive: r.get(4)?,
                    watch: r.get(5)?,
                    excludes: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                },
            ))
        },
    )?)
}
pub fn set_state(
    c: &Connection,
    id: &str,
    availability: &str,
    freshness: &str,
    message: Option<&str>,
) -> Result<()> {
    c.execute(
        "UPDATE scopes SET availability=?,freshness=?,message=? WHERE id=?",
        params![availability, freshness, message, id],
    )?;
    bump(c)
}
pub fn cleanup(c: &Connection) -> Result<()> {
    c.execute("DELETE FROM entries WHERE NOT EXISTS (SELECT 1 FROM memberships m WHERE m.entry_id=entries.id)",[])?;
    c.execute("DELETE FROM directories WHERE NOT EXISTS (SELECT 1 FROM entries e WHERE e.dir_id=directories.id)",[])?;
    Ok(())
}
fn visibility_sql(hidden: bool) -> &'static str {
    if hidden {
        "(e.hidden=1 OR d.hidden=1)"
    } else {
        "e.hidden=0 AND d.hidden=0"
    }
}
pub fn summary_for(c: &Connection, scope_id: &str, hidden: bool) -> Result<Summary> {
    fn buckets(c: &Connection, col: &str, hidden: bool, scope_id: &str) -> Result<Vec<Bucket>> {
        let visibility = visibility_sql(hidden);
        let mut s=c.prepare(&format!("SELECT {col},COUNT(*) FROM entries e JOIN directories d ON d.id=e.dir_id WHERE {visibility} AND (?='' OR EXISTS(SELECT 1 FROM memberships m WHERE m.entry_id=e.id AND m.scope_id=?)) GROUP BY {col} ORDER BY COUNT(*) DESC,{col}"))?;
        let result = s
            .query_map(params![scope_id, scope_id], |r| {
                Ok(Bucket {
                    name: r.get(0)?,
                    count: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(result)
    }
    Ok(Summary {
        hidden_directories: crate::hidden::list(c)?,
        scan_runs: Vec::new(),
        scan_paused: false,
        facet_scope_id: scope_id.into(),
        facet_hidden: hidden,
        scopes: scopes(c)?,
        total: c.query_row("SELECT COUNT(*) FROM entries e JOIN directories d ON d.id=e.dir_id WHERE e.hidden=0 AND d.hidden=0", [], |r| {
            r.get(0)
        })?,
        hidden: c.query_row("SELECT COUNT(*) FROM entries e JOIN directories d ON d.id=e.dir_id WHERE e.hidden=1 OR d.hidden=1", [], |r| {
            r.get(0)
        })?,
        size: c.query_row(
            "SELECT COALESCE(SUM(size),0) FROM entries e JOIN directories d ON d.id=e.dir_id WHERE e.hidden=0 AND d.hidden=0",
            [],
            |r| r.get(0),
        )?,
        extensions: buckets(c, "extension", false, scope_id)?,
        hidden_extensions: buckets(c, "extension", true, scope_id)?,
        groups: buckets(c, "group_name", hidden, scope_id)?,
        revision: revision(c)?,
    })
}
pub fn query(c: &Connection, q: &Query) -> Result<QueryResult> {
    let mut conditions = vec![visibility_sql(q.hidden).to_string()];
    let mut values: Vec<Value> = vec![];
    if let Some(ids) = &q.candidate_ids {
        conditions.push("e.id IN (SELECT CAST(value AS INTEGER) FROM json_each(?))".into());
        values.push(serde_json::to_string(ids)?.into());
    }
    if !q.scope_id.is_empty() {
        conditions.push(
            "EXISTS(SELECT 1 FROM memberships m WHERE m.entry_id=e.id AND m.scope_id=?)".into(),
        );
        values.push(q.scope_id.clone().into());
    }
    if !q.extension.is_empty() {
        conditions.push("e.extension=?".into());
        values.push(
            if q.extension == "__none__" {
                ""
            } else {
                q.extension.trim_start_matches('.')
            }
            .to_lowercase()
            .into(),
        );
    }
    if !q.group.is_empty() {
        conditions.push("e.group_name=?".into());
        values.push(q.group.clone().into());
    }
    if !q.search.is_empty() {
        let names = "(instr(lower(e.name),lower(?))>0 OR instr(lower(d.display),lower(?))>0)";
        if q.search_mode == "content" {
            conditions.push(crate::content::match_sql(q, &mut values));
        } else {
            values.push(q.search.clone().into());
            values.push(q.search.clone().into());
            if q.search_mode == "all" {
                let content = crate::content::match_sql(q, &mut values);
                conditions.push(format!("({names} OR {content})"));
            } else {
                conditions.push(names.into());
            }
        }
    }
    if let Some(size) = q.min_size {
        conditions.push("e.size>=?".into());
        values.push(Value::Integer(size.min(i64::MAX as u64) as i64));
    }
    if let Some(time) = q.modified_after {
        conditions.push("e.mtime>=?".into());
        values.push(Value::Integer(time));
    }
    let from = format!(
        "FROM entries e JOIN directories d ON d.id=e.dir_id WHERE {}",
        conditions.join(" AND ")
    );
    let total = c.query_row(
        &format!("SELECT COUNT(*) {from}"),
        params_from_iter(values.iter()),
        |r| r.get(0),
    )?;
    let sort = match q.sort.as_str() {
        "size" => "e.size",
        "mtime" => "e.mtime",
        "extension" => "e.extension",
        "directory" => "d.display COLLATE NOCASE",
        _ => "e.name COLLATE NOCASE",
    };
    let direction = if q.descending { "DESC" } else { "ASC" };
    let limit = if q.limit == 0 { 100 } else { q.limit.min(500) };
    values.push(Value::Integer(limit as i64));
    values.push(Value::Integer(q.offset.min(i64::MAX as usize) as i64));
    let sql=format!("SELECT e.id,e.name,d.path,e.native_name,e.extension,e.group_name,e.size,e.mtime,(e.hidden OR d.hidden),e.attributes,
      (SELECT s.id FROM scopes s JOIN memberships m ON s.id=m.scope_id WHERE m.entry_id=e.id ORDER BY s.depth DESC,s.id LIMIT 1),
      EXISTS(SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=e.id AND s.availability='available'),d.hidden
      {from} ORDER BY {sort} {direction},e.id ASC LIMIT ? OFFSET ?");
    let mut stmt = c.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(values.iter()))?;
    let mut entries = Vec::new();
    while let Some(r) = rows.next()? {
        let directory = PathBuf::from(decode(&r.get::<_, Vec<u8>>(2)?));
        let native_name = decode(&r.get::<_, Vec<u8>>(3)?);
        let scope_id: String = r.get(10)?;
        let (root_bytes, scope_name): (Vec<u8>, String) = c
            .prepare_cached("SELECT root,name FROM scopes WHERE id=?")?
            .query_row([&scope_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let scope_root = PathBuf::from(decode(&root_bytes));
        let scope_name = display_path_text(&scope_name);
        let relative = directory.strip_prefix(&scope_root).unwrap_or(&directory);
        entries.push(Entry {
            content_ready: c.query_row("SELECT EXISTS(SELECT 1 FROM content_items ci WHERE ci.entry_id=? AND ci.state='ready')", [r.get::<_,i64>(0)?], |r|r.get(0))?,
            snippet: None,
            id: r.get::<_, i64>(0)?.to_string(),
            name: r.get(1)?,
            path: display_path(&directory.join(native_name)),
            directory: scoped_directory(&scope_name, relative),
            directory_path: display_path(&directory),
            extension: r.get(4)?,
            group: r.get(5)?,
            size: r.get(6)?,
            mtime: r.get(7)?,
            hidden: r.get(8)?,
            directory_hidden: r.get(12)?,
            placeholder: placeholder(r.get::<_, u32>(9)?),
            scope_id,
            scope_name,
            online: r.get(11)?,
        });
    }
    let content_search =
        !q.search.is_empty() && ["content", "all"].contains(&q.search_mode.as_str());
    if content_search {
        for entry in &mut entries {
            let text: Option<(String,bool)> = c.query_row("SELECT substr(ci.text,max(1,instr(lower(ci.text),lower(?))-45),length(?)+190),ci.truncated FROM content_items ci JOIN entries e ON e.id=ci.entry_id WHERE e.id=? AND ci.state='ready' AND ci.identity=e.identity AND ci.size=e.size AND ci.mtime=e.mtime AND EXISTS(SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=e.id AND s.content_enabled=1 AND (?='' OR s.id=?))", params![q.search,q.search,entry.id,q.scope_id,q.scope_id], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((text, truncated)) = text {
                entry.snippet = crate::content::snippet(&text, &q.search, truncated);
            }
        }
    }

    Ok(QueryResult {
        search_engine: if content_search { "content" } else { "local" }.into(),
        search_notice: if content_search {
            Some(
                if q.search.chars().count() < 3 {
                    "正在搜索已索引内容 · 短词搜索可能较慢；未完成索引的文件暂不参与内容匹配"
                } else {
                    "正在搜索已索引内容 · 未完成索引的文件暂不参与内容匹配"
                }
                .into(),
            )
        } else {
            None
        },
        entries,
        total,
        revision: revision(c)?,
        offset: q.offset,
    })
}
pub fn entry_path(c: &Connection, id: &str) -> Result<(PathBuf, String, u64, i64, u32)> {
    let row=c.query_row("SELECT d.path,e.native_name,e.identity,e.size,e.mtime,e.attributes FROM entries e JOIN directories d ON e.dir_id=d.id WHERE e.id=?",[id],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Vec<u8>>(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    match row {
        Some((dir, name, identity, size, mtime, attrs)) => Ok((
            PathBuf::from(decode(&dir)).join(decode(&name)),
            identity,
            size,
            mtime,
            attrs,
        )),
        None => bail!("文件索引已失效，请刷新后重试。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_directory_migration_preserves_individual_hidden_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.sqlite");
        let c = connect(&path).unwrap();
        c.execute_batch("DROP INDEX directories_hidden; ALTER TABLE directories DROP COLUMN hidden;
            DROP TABLE hidden_directories; PRAGMA user_version=4;
            INSERT INTO directories(id,path,display) VALUES(1,x'01','old');
            INSERT INTO entries(dir_id,native_name,name,extension,group_name,size,mtime,identity,attributes,hidden)
                VALUES(1,x'01','old.txt','txt','文档',1,1,'test',0,1);").unwrap();
        drop(c);
        let c = connect(&path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert!(c
            .query_row("SELECT hidden FROM entries", [], |r| r.get::<_, bool>(0))
            .unwrap());
        assert!(!c
            .query_row("SELECT hidden FROM directories", [], |r| r
                .get::<_, bool>(0))
            .unwrap());
        assert!(crate::hidden::list(&c).unwrap().is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn legacy_drive_scope_displays_normally_without_rewriting_native_paths() {
        let temp = tempfile::tempdir().unwrap();
        let c = connect(&temp.path().join("index.sqlite")).unwrap();
        let native_root = encode(Path::new(r"\\?\D:\").as_os_str());
        let native_dir = encode(Path::new(r"\\?\D:\中文资料").as_os_str());
        c.execute(
            "INSERT INTO scopes(id,root,path,name,identity,depth,recursive,watch,excludes)
             VALUES('drive',?,? ,?,'test-identity',1,1,0,'[]')",
            params![native_root, r"\\?\D:\", r"\\?\D:\"],
        )
        .unwrap();
        c.execute(
            "INSERT INTO directories(id,path,display) VALUES(1,?,?)",
            params![native_dir, r"\\?\D:\中文资料"],
        )
        .unwrap();
        c.execute(
            "INSERT INTO entries(id,dir_id,native_name,name,extension,group_name,size,mtime,identity,attributes)
             VALUES(1,1,?,'项目 📄.md','md','文档',8,0,'test-file',0)",
            [encode(Path::new("项目 📄.md").as_os_str())],
        ).unwrap();
        c.execute(
            "INSERT INTO memberships(scope_id,entry_id,generation) VALUES('drive',1,'test')",
            [],
        )
        .unwrap();
        let scope = scopes(&c).unwrap().remove(0);
        assert_eq!(scope.name, r"D:\");
        assert_eq!(scope.path, r"D:\");
        assert_eq!(root(&c, "drive").unwrap().3.path, r"D:\");
        let entry = query(&c, &Query::default()).unwrap().entries.remove(0);
        assert_eq!(entry.path, r"D:\中文资料\项目 📄.md");
        assert_eq!(entry.directory, r"D:\中文资料");
        assert_eq!(entry.scope_name, r"D:\");
        assert_eq!(
            entry_path(&c, "1").unwrap().0,
            Path::new(r"\\?\D:\中文资料\项目 📄.md")
        );
        let after: Vec<u8> = c
            .query_row("SELECT root FROM scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(native_root, after);
    }
}
