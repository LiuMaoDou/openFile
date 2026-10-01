//! Durable per-folder content indexing. FTS and file references share transactions.
use crate::{db, extract, model::*, platform::*, Engine};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

// Reserve original text plus at most three bytes of encoded tokens per byte.
const CONTENT_BUFFER_BUDGET: usize = 128 * 1024 * 1024;
const COMMIT_TEXT_BUDGET: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct ContentBudget {
    used: Mutex<usize>,
    available: Condvar,
}
struct ContentPermit {
    budget: Arc<ContentBudget>,
    bytes: usize,
}
impl ContentBudget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> ContentPermit {
        assert!(bytes <= CONTENT_BUFFER_BUDGET);
        let mut used = self.used.lock().unwrap();
        while *used + bytes > CONTENT_BUFFER_BUDGET {
            used = self.available.wait(used).unwrap();
        }
        *used += bytes;
        ContentPermit {
            budget: self.clone(),
            bytes,
        }
    }
}
impl Drop for ContentPermit {
    fn drop(&mut self) {
        *self.budget.used.lock().unwrap() -= self.bytes;
        self.budget.available.notify_all();
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub before: String,
    pub matched: String,
    pub after: String,
    pub truncated: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentScope {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub paused: bool,
    pub online: bool,
    pub eligible: u64,
    pub ready: u64,
    pub pending: u64,
    pub failed: u64,
    pub skipped: u64,
    pub truncated: u64,
    pub unsupported: u64,
    pub last_indexed: Option<i64>,
}
#[derive(Serialize)]
pub struct ContentIssue {
    pub name: String,
    pub message: String,
}
#[derive(Serialize)]
pub struct ContentStatus {
    pub scopes: Vec<ContentScope>,
    pub issues: Vec<ContentIssue>,
}
const EXTENSIONS: &str = "'pdf','docx','xlsx','pptx','txt','md','markdown','csv','tsv','log','rs','ts','tsx','js','jsx','json','toml','yaml','yml','xml','html','htm','css','scss','py','go','c','cpp','h','hpp','java','kt','sh','bash','zsh','sql','ini','conf','cfg','bat','ps1','vue','svelte','tex','rb','swift'";

pub(crate) fn migrate(c: &Connection) -> Result<()> {
    let columns: Vec<String> = c
        .prepare("PRAGMA table_info(scopes)")?
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    c.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        if !columns.iter().any(|s| s == "content_enabled") {
            c.execute_batch(
                "ALTER TABLE scopes ADD COLUMN content_enabled INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE scopes ADD COLUMN content_paused INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE scopes ADD COLUMN content_epoch INTEGER NOT NULL DEFAULT 1;",
            )?;
        }
        c.execute_batch("CREATE TABLE IF NOT EXISTS content_items(
            entry_id INTEGER PRIMARY KEY REFERENCES entries(id) ON DELETE CASCADE,
            identity TEXT NOT NULL,size INTEGER NOT NULL,mtime INTEGER NOT NULL,
            state TEXT NOT NULL,text TEXT NOT NULL DEFAULT '',message TEXT,truncated INTEGER NOT NULL DEFAULT 0,indexed_at INTEGER NOT NULL);
          CREATE VIRTUAL TABLE IF NOT EXISTS content_fts USING fts5(text,content='content_items',content_rowid='entry_id',tokenize='trigram');
          CREATE TRIGGER IF NOT EXISTS content_insert AFTER INSERT ON content_items BEGIN
            INSERT INTO content_fts(rowid,text) VALUES(new.entry_id,new.text); END;
          CREATE TRIGGER IF NOT EXISTS content_delete AFTER DELETE ON content_items BEGIN
            INSERT INTO content_fts(content_fts,rowid,text) VALUES('delete',old.entry_id,old.text); END;
          CREATE TRIGGER IF NOT EXISTS content_update AFTER UPDATE ON content_items BEGIN
            INSERT INTO content_fts(content_fts,rowid,text) VALUES('delete',old.entry_id,old.text);
            INSERT INTO content_fts(rowid,text) VALUES(new.entry_id,new.text); END;
          CREATE TRIGGER IF NOT EXISTS content_entry_changed AFTER UPDATE ON entries
            WHEN old.identity!=new.identity OR old.size!=new.size OR old.mtime!=new.mtime OR old.dir_id!=new.dir_id OR old.native_name!=new.native_name OR old.attributes!=new.attributes BEGIN
            DELETE FROM content_items WHERE entry_id=new.id; END;
          CREATE TRIGGER IF NOT EXISTS content_scope_removed AFTER DELETE ON scopes BEGIN
            DELETE FROM content_items WHERE NOT EXISTS (SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=content_items.entry_id AND s.content_enabled=1); END;
          CREATE TRIGGER IF NOT EXISTS content_scope_disabled AFTER UPDATE OF content_enabled ON scopes WHEN new.content_enabled=0 BEGIN
            DELETE FROM content_items WHERE NOT EXISTS (SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=content_items.entry_id AND s.content_enabled=1); END;
          ")?;
        c.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS content_words USING fts5(tokens,content='',contentless_delete=1);
          CREATE TRIGGER IF NOT EXISTS content_words_delete AFTER DELETE ON content_items BEGIN
            DELETE FROM content_words WHERE rowid=old.entry_id; END;
          CREATE TRIGGER IF NOT EXISTS content_words_update AFTER UPDATE OF text ON content_items BEGIN
            DELETE FROM content_words WHERE rowid=old.entry_id; END;")?;
        let content_columns = c
            .prepare("PRAGMA table_info(content_items)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !content_columns.iter().any(|name| name == "text_limit") {
            // Additive metadata remains readable by old v5 clients. Keep old
            // prefixes searchable until their replacement commits successfully.
            c.execute_batch(
                "ALTER TABLE content_items ADD COLUMN text_limit INTEGER NOT NULL DEFAULT 2097152;",
            )?;
        }
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 4 {
            c.execute_batch("PRAGMA user_version=4;")?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => c.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = c.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    Ok(())
}
// Every content hit is joined back to a current file and an enabled folder.
pub(crate) fn match_sql(query: &Query, values: &mut Vec<rusqlite::types::Value>) -> String {
    let mut conditions = vec!["ci.entry_id=e.id AND ci.state='ready' AND ci.identity=e.identity AND ci.size=e.size AND ci.mtime=e.mtime".to_string()];
    conditions.push("EXISTS(SELECT 1 FROM memberships cm JOIN scopes cs ON cs.id=cm.scope_id WHERE cm.entry_id=e.id AND cs.content_enabled=1 AND (?='' OR cs.id=?))".into());
    values.push(query.scope_id.clone().into());
    values.push(query.scope_id.clone().into());
    if query.match_mode == "keywords" {
        let terms = crate::search::terms(&query.search);
        if terms.is_empty() {
            conditions.push("0".into());
        } else {
            values.push(crate::search::expression(&terms).into());
            // Literal term fallback keeps compound words and technical substrings
            // searchable when dictionary boundaries differ or backfill is pending.
            let literal = terms
                .iter()
                .map(|term| {
                    values.push(term.clone().into());
                    "instr(lower(ci.text),lower(?))>0"
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            conditions.push(format!("(ci.entry_id IN (SELECT rowid FROM content_words WHERE content_words MATCH ?) OR ({literal}))"));
        }
        return format!(
            "EXISTS(SELECT 1 FROM content_items ci WHERE {})",
            conditions.join(" AND ")
        );
    }
    // FTS5 trigram cannot match one or two characters; those use the stored text.
    if query.search.chars().count() >= 3 {
        conditions.push(
            "ci.entry_id IN (SELECT rowid FROM content_fts WHERE content_fts MATCH ?)".into(),
        );
        values.push(format!("\"{}\"", query.search.replace('"', "\"\"")).into());
    }
    conditions.push("instr(lower(ci.text),lower(?))>0".into());
    values.push(query.search.clone().into());
    format!(
        "EXISTS(SELECT 1 FROM content_items ci WHERE {})",
        conditions.join(" AND ")
    )
}
pub(crate) fn snippet(text: &str, search: &str, truncated: bool) -> Option<Snippet> {
    if search.is_empty() {
        return None;
    }
    let at = text
        .to_ascii_lowercase()
        .find(&search.to_ascii_lowercase())?;
    let before: String = text[..at]
        .chars()
        .rev()
        .take(45)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let end = at + search.len();
    Some(Snippet {
        before,
        matched: text[at..end].into(),
        after: text[end..].chars().take(100).collect(),
        truncated,
    })
}
#[derive(Clone)]
struct Job {
    id: String,
    scope: String,
    version: i64,
    epoch: i64,
}
struct PreparedContent {
    job: Job,
    initial: (std::path::PathBuf, String, u64, i64, u32),
    output: Result<extract::Extracted>,
    keywords: Option<String>,
    words_only: bool,
    permit: Option<ContentPermit>,
}
impl Engine {
    pub(crate) fn start_content_worker(&self) {
        if cfg!(test) {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        thread::spawn(move || {
            use rayon::prelude::*;
            let mut pool = rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .unwrap();
            let mut after_id = 0i64;
            let mut exhausted_revision = None;
            let budget = Arc::new(ContentBudget::default());
            loop {
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let engine = Engine { inner };
                let desired = engine.performance().map_or(2, |s| s.content_threads);
                if pool.current_num_threads() != desired {
                    if let Ok(next) = rayon::ThreadPoolBuilder::new().num_threads(desired).build() {
                        pool = next;
                    }
                }
                // Once caught up, only a committed change can introduce work.
                // Read the revision before selecting: a concurrent commit then
                // causes another pass, never a missed wakeup.
                let revision = engine
                    .inner
                    .content_db
                    .lock()
                    .ok()
                    .and_then(|c| db::revision(&c).ok());
                if revision.is_some() && revision == exhausted_revision {
                    drop(engine);
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
                let jobs = match engine.next_content_jobs_after(after_id, 128) {
                    Ok(jobs) => jobs,
                    Err(_) => {
                        drop(engine);
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                };
                if jobs.is_empty() {
                    exhausted_revision = if after_id == 0 { revision } else { None };
                    after_id = 0;
                    drop(engine);
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
                after_id = jobs.last().and_then(|j| j.id.parse().ok()).unwrap_or(0);
                // Reserve bytes before parsing and retain the permit through
                // commit. A slow large document no longer holds up ready ones.
                thread::scope(|scope| {
                    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                    let engine = &engine;
                    let pool = &pool;
                    let budget = &budget;
                    scope.spawn(move || {
                        pool.install(|| {
                            jobs.par_iter().for_each(|job| {
                                if let Ok(Some(prepared)) =
                                    engine.prepare_content_job(job, Some(budget))
                                {
                                    let _ = sender.send(prepared);
                                }
                            })
                        })
                    });
                    while let Ok(first) = receiver.recv() {
                        let mut bytes = first.output.as_ref().map_or(0, |v| v.text.len());
                        let mut prepared = vec![first];
                        while prepared.len() < 8 && bytes < COMMIT_TEXT_BUDGET {
                            let Ok(next) = receiver.try_recv() else {
                                break;
                            };
                            bytes += next.output.as_ref().map_or(0, |v| v.text.len());
                            prepared.push(next);
                        }
                        let _ = engine.commit_content_jobs(prepared);
                    }
                });
            }
        });
    }
    #[cfg(test)]
    fn next_content_job(&self) -> Result<Option<Job>> {
        Ok(self.next_content_jobs_after(0, 1)?.into_iter().next())
    }
    fn next_content_jobs_after(&self, after_id: i64, limit: usize) -> Result<Vec<Job>> {
        let c = self.inner.content_db.lock().unwrap();
        let enabled: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM scopes WHERE content_enabled=1 AND content_paused=0 AND availability='available' AND freshness IN ('current','partial'))", [], |r| r.get(0))?;
        if !enabled {
            return Ok(Vec::new());
        }
        // Iterate the rowid range rather than sorting all extension candidates
        // for every single job. Choose one eligible owner per entry locally.
        let sql = format!("SELECT e.id,s.id,s.config_version,s.content_epoch FROM entries e NOT INDEXED
            JOIN scopes s ON s.id=(SELECT cs.id FROM memberships m JOIN scopes cs ON cs.id=m.scope_id
                WHERE m.entry_id=e.id AND cs.content_enabled=1 AND cs.content_paused=0
                AND cs.availability='available' AND cs.freshness IN ('current','partial')
                ORDER BY cs.depth DESC,cs.id LIMIT 1)
            WHERE e.id>? AND e.extension IN ({EXTENSIONS})
                AND NOT EXISTS(SELECT 1 FROM content_items ci WHERE ci.entry_id=e.id
                    AND NOT(ci.truncated=1 AND ci.text_limit<{})
                    AND (ci.state!='ready' OR EXISTS(SELECT 1 FROM content_words WHERE rowid=e.id)))
            ORDER BY e.id LIMIT ?", extract::MAX_TEXT);
        let mut statement = c.prepare_cached(&sql)?;
        let jobs = statement
            .query_map(params![after_id, limit as i64], |r| {
                Ok(Job {
                    id: r.get::<_, i64>(0)?.to_string(),
                    scope: r.get(1)?,
                    version: r.get(2)?,
                    epoch: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(jobs)
    }
    fn valid_content_job(&self, job: &Job) -> bool {
        self.lock().and_then(|c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=? AND s.id=? AND s.config_version=? AND s.content_epoch=? AND s.content_enabled=1 AND s.content_paused=0 AND s.availability='available')",params![job.id,job.scope,job.version,job.epoch],|r|r.get::<_,bool>(0))?)).unwrap_or(false)
    }
    fn checked_content_job(&self, job: &Job) -> Result<extract::Request> {
        let (path, expected, size, time) = self.checked_entry(&job.id)?;
        let (root, root_identity, version, input) = {
            let c = self.lock()?;
            db::root(&c, &job.scope)?
        };
        if identity(&root, &std::fs::metadata(&root)?)? != root_identity {
            bail!("内容索引根文件夹已变化");
        }
        let relative = path
            .strip_prefix(root)
            .context("文件已离开启用内容索引的文件夹")?;
        if version != job.version
            || !input.content_enabled
            || (!input.recursive && relative.components().count() != 1)
            || crate::rules::Excludes::new(&input.excludes)?.matches(relative)
            || !self.valid_content_job(job)
        {
            bail!("内容索引配置已变化");
        }
        Ok(extract::Request {
            path: encode(path.as_os_str()),
            identity: expected,
            size,
            mtime: time,
        })
    }
    fn prepare_content_job(
        &self,
        job: &Job,
        budget: Option<&Arc<ContentBudget>>,
    ) -> Result<Option<PreparedContent>> {
        let initial = {
            let c = self.lock()?;
            db::entry_path(&c, &job.id)?
        };
        let mut permit = None;
        let mut words_only = false;
        let output = (|| {
            let request = self.checked_content_job(job)?;
            if request.size > extract::MAX_FILE {
                bail!("文件超过 64 MiB，已跳过内容索引");
            }
            if let Some(budget) = budget {
                let path = std::path::PathBuf::from(decode(&request.path));
                permit = Some(budget.reserve(
                    4 * extract::text_budget(
                        request.size,
                        &extension(&path.file_name().unwrap_or_default().to_string_lossy()),
                    ),
                ));
            }
            // Reuse validated stored text when only the word index is missing.
            let cached = self.inner.content_db.lock().unwrap().query_row(
                "SELECT text,truncated,message FROM content_items WHERE entry_id=? AND state='ready' AND identity=? AND size=? AND mtime=? AND NOT(truncated=1 AND text_limit<?)",
                params![job.id,request.identity,request.size as i64,request.mtime,extract::MAX_TEXT as i64],
                |r| Ok(extract::Extracted { text:r.get(0)?,truncated:r.get(1)?,note:r.get(2)? })).optional()?;
            let result = match cached {
                Some(result) => {
                    words_only = true;
                    result
                }
                None => extract::isolated(&request, || self.valid_content_job(job))?,
            };
            self.checked_content_job(job)?;
            Ok(result)
        })();
        if !self.valid_content_job(job) {
            return Ok(None);
        }
        let keywords = output
            .as_ref()
            .ok()
            .map(|v| crate::search::index_text(&v.text));
        Ok(Some(PreparedContent {
            job: job.clone(),
            initial,
            output,
            keywords,
            words_only,
            permit,
        }))
    }
    #[cfg(test)]
    fn index_content_job(&self, job: &Job) -> Result<()> {
        self.commit_content_jobs(self.prepare_content_job(job, None)?.into_iter().collect())
    }
    fn commit_content_jobs(&self, prepared: Vec<PreparedContent>) -> Result<()> {
        if prepared.is_empty() {
            return Ok(());
        }
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        let mut changed = false;
        for PreparedContent {
            job,
            initial,
            output,
            keywords,
            words_only,
            permit: _permit,
        } in prepared
        {
            let Ok(current) = db::entry_path(&tx, &job.id) else {
                continue;
            };
            if initial != current {
                continue;
            }
            let still_valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=? AND s.id=? AND s.content_enabled=1 AND s.content_paused=0 AND s.availability='available' AND s.config_version=? AND s.content_epoch=?)", params![job.id,job.scope,job.version,job.epoch], |r| r.get(0))?;
            if !still_valid {
                continue;
            }
            if words_only {
                if let Some(tokens) = keywords {
                    tx.execute("DELETE FROM content_words WHERE rowid=?", [&job.id])?;
                    tx.execute(
                        "INSERT INTO content_words(rowid,tokens) VALUES(?,?)",
                        params![job.id, tokens],
                    )?;
                    changed = true;
                }
                continue;
            }
            let (state, text, message, truncated) = match output {
                Ok(v) => ("ready", v.text, v.note, v.truncated),
                Err(e) => {
                    let message = format!("{e:#}");
                    // Failed expansion must not discard the previously usable
                    // prefix or retry indefinitely on every worker pass.
                    if tx.execute("UPDATE content_items SET text_limit=?,message=? WHERE entry_id=? AND state='ready' AND truncated=1 AND text_limit<?",
                        params![extract::MAX_TEXT as i64,format!("扩展索引失败，保留原有部分内容：{message}"),job.id,extract::MAX_TEXT as i64])? > 0 {
                        changed = true;
                        continue;
                    }
                    let skipped = initial.2 > extract::MAX_FILE
                        || message.contains("未提取到文字")
                        || message.contains("占位")
                        || message.contains("链接");
                    (
                        if skipped { "skipped" } else { "failed" },
                        String::new(),
                        Some(message),
                        false,
                    )
                }
            };
            tx.execute("INSERT INTO content_items(entry_id,identity,size,mtime,state,text,message,truncated,indexed_at,text_limit) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(entry_id) DO UPDATE SET identity=excluded.identity,size=excluded.size,mtime=excluded.mtime,state=excluded.state,text=excluded.text,message=excluded.message,truncated=excluded.truncated,indexed_at=excluded.indexed_at,text_limit=excluded.text_limit", params![job.id,current.1,current.2 as i64,current.3,state,text,message,truncated,now(),extract::MAX_TEXT as i64])?;
            if let Some(tokens) = keywords {
                tx.execute(
                    "INSERT INTO content_words(rowid,tokens) VALUES(?,?)",
                    params![job.id, tokens],
                )?;
            }
            changed = true;
        }
        if changed {
            db::bump(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn content_action(&self, id: &str, action: &str) -> Result<()> {
        if !["enable", "disable", "pause", "resume", "rebuild"].contains(&action) {
            bail!("未知内容索引操作");
        }
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        let enabled: bool = tx
            .query_row("SELECT content_enabled FROM scopes WHERE id=?", [id], |r| {
                r.get(0)
            })
            .context("文件夹不存在")?;
        if action == "rebuild" && !enabled {
            bail!("请先启用这个文件夹的内容索引");
        }
        match action {
            "enable" => {
                tx.execute("UPDATE scopes SET content_enabled=1,content_paused=0,content_epoch=content_epoch+1 WHERE id=?", [id])?;
            }
            "disable" => {
                tx.execute("UPDATE scopes SET content_enabled=0,content_paused=0,content_epoch=content_epoch+1 WHERE id=?", [id])?;
            }
            "pause" | "resume" => {
                tx.execute(
                    "UPDATE scopes SET content_paused=?,content_epoch=content_epoch+1 WHERE id=?",
                    params![action == "pause", id],
                )?;
            }
            "rebuild" => {
                tx.execute(
                    "UPDATE scopes SET content_paused=0,content_epoch=content_epoch+1 WHERE id=?",
                    [id],
                )?;
                tx.execute("DELETE FROM content_items WHERE entry_id IN (SELECT entry_id FROM memberships WHERE scope_id=?)", [id])?;
            }
            _ => unreachable!(),
        }
        db::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
    pub fn content_status(&self, scope_id: &str) -> Result<ContentStatus> {
        let reader = self.status_reader()?;
        let c = reader.unchecked_transaction()?;
        let scopes = db::scopes(&c)?;
        let mut result = Vec::new();
        for scope in scopes
            .into_iter()
            .filter(|s| scope_id.is_empty() || s.id == scope_id)
        {
            let sql=format!("SELECT COUNT(*),COALESCE(SUM(ci.state='ready' AND NOT(ci.truncated=1 AND ci.text_limit<{})
                    AND EXISTS(SELECT 1 FROM content_words WHERE rowid=e.id)),0),COALESCE(SUM(ci.state='failed'),0),COALESCE(SUM(ci.state='skipped'),0),COALESCE(SUM(ci.truncated),0),MAX(ci.indexed_at) FROM entries e JOIN memberships m ON m.entry_id=e.id LEFT JOIN content_items ci ON ci.entry_id=e.id WHERE m.scope_id=? AND e.extension IN ({EXTENSIONS})",extract::MAX_TEXT);
            let (eligible, ready, failed, skipped, truncated, last_indexed): (
                u64,
                u64,
                u64,
                u64,
                u64,
                Option<i64>,
            ) = c.query_row(&sql, [&scope.id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?;
            result.push(ContentScope {
                id: scope.id,
                name: scope.name,
                enabled: scope.content_enabled,
                paused: scope.content_paused,
                online: scope.availability == "available",
                eligible,
                ready,
                pending: eligible.saturating_sub(ready + failed + skipped),
                failed,
                skipped,
                truncated,
                unsupported: scope.count.saturating_sub(eligible),
                last_indexed,
            });
        }
        let mut stmt = c.prepare("SELECT e.name,ci.message FROM content_items ci JOIN entries e ON e.id=ci.entry_id WHERE ci.message IS NOT NULL AND EXISTS(SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=e.id AND s.content_enabled=1 AND (?='' OR s.id=?)) ORDER BY ci.indexed_at DESC LIMIT 20")?;
        let issues = stmt
            .query_map(params![scope_id, scope_id], |r| {
                Ok(ContentIssue {
                    name: r.get(0)?,
                    message: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ContentStatus {
            scopes: result,
            issues,
        })
    }
    pub fn content_preview(&self, id: &str, search: &str) -> Result<TextPreview> {
        self.checked_entry(id)?;
        let c = self
            .inner
            .query_db
            .lock()
            .map_err(|_| anyhow::anyhow!("索引读取不可用"))?;
        // SQLite offsets count Unicode characters. Return at most 16K characters
        // (64 KiB UTF-8), without copying the complete document into Rust/UI.
        c.query_row("WITH selected AS MATERIALIZED (
            SELECT ci.entry_id,max(1,instr(lower(ci.text),lower(?))-1024) AS start
            FROM content_items ci JOIN entries e ON e.id=ci.entry_id
            WHERE e.id=? AND ci.state='ready' AND ci.identity=e.identity AND ci.size=e.size AND ci.mtime=e.mtime
            AND EXISTS(SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=e.id AND s.content_enabled=1))
            SELECT substr(ci.text,start,16384),ci.truncated OR start>1 OR length(ci.text)>start+16383
            FROM selected JOIN content_items ci ON ci.entry_id=selected.entry_id",
            params![search,id], |r| Ok(TextPreview { text:r.get(0)?,truncated:r.get(1)? }))
            .context("此文件尚未完成内容索引，请先启用所在文件夹的内容索引")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, time::Instant};
    fn input(path: &Path, enabled: bool) -> ScopeInput {
        ScopeInput {
            path: path.to_string_lossy().into(),
            recursive: true,
            watch: false,
            excludes: vec![],
            content_enabled: enabled,
        }
    }
    fn wait_scan(e: &Engine) {
        let start = Instant::now();
        while e
            .summary()
            .unwrap()
            .scopes
            .iter()
            .any(|s| !["current", "partial"].contains(&s.freshness.as_str()))
        {
            assert!(start.elapsed() < Duration::from_secs(15));
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn index(e: &Engine) {
        wait_scan(e);
        for _ in 0..100 {
            match e.next_content_job().unwrap() {
                Some(job) => e.index_content_job(&job).unwrap(),
                None => return,
            }
        }
        panic!("index did not finish");
    }
    fn hits(e: &Engine, text: &str, mode: &str, scope: &str) -> QueryResult {
        e.query(&Query {
            search: text.into(),
            search_mode: mode.into(),
            scope_id: scope.into(),
            ..Default::default()
        })
        .unwrap()
    }
    #[test]
    fn keyword_search_names_content_ranking_and_backfill() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("2026年项目合同_最终版.txt"),
            "项目合同已经确认。付款需要审批。AB-123 使用 C++ 开发。",
        )
        .unwrap();
        fs::write(root.join("合同.txt"), "合同 付款").unwrap();
        fs::write(root.join("其他.txt"), "仅有合同，没有另外的关键词").unwrap();
        let e = Engine::open(temp.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, true)).unwrap();
        index(&e);
        let query = |search: &str, mode: &str| Query {
            search: search.into(),
            search_mode: mode.into(),
            match_mode: "keywords".into(),
            sort: "relevance".into(),
            scope_id: scope.clone(),
            ..Default::default()
        };
        let q = query("合同 付款", "content");
        let result = e.query(&q).unwrap();
        assert_eq!(result.total, 2);
        assert_eq!(result.entries[0].name, "合同.txt");
        assert!(result
            .entries
            .iter()
            .all(|v| v
                .snippet
                .as_ref()
                .is_some_and(|s| s.before.chars().count() <= 4)));
        let hidden_id = result.entries[0].id.clone();
        e.set_hidden(std::slice::from_ref(&hidden_id), true)
            .unwrap();
        assert_eq!(e.query(&q).unwrap().total, 1);
        assert_eq!(
            e.query(&Query {
                hidden: true,
                ..q.clone()
            })
            .unwrap()
            .total,
            1
        );
        e.set_hidden(&[hidden_id], false).unwrap();
        assert!(e
            .query(&Query {
                match_mode: "unknown".into(),
                ..q.clone()
            })
            .is_err());
        let many = (0..33)
            .map(|n| format!("term{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(e.query(&query(&many, "content")).is_err());
        assert_eq!(hits(&e, "合同 付款", "content", &scope).total, 1);
        assert_eq!(e.query(&query("项目付款", "content")).unwrap().total, 1);
        assert_eq!(e.query(&query("合同 不存在", "content")).unwrap().total, 0);
        assert_eq!(e.query(&query("2026 最终", "all")).unwrap().total, 1);
        assert_eq!(
            e.query(&query("2026 合同 最终版 txt", "name"))
                .unwrap()
                .total,
            1
        );
        assert_eq!(e.query(&query("项目合同", "content")).unwrap().total, 1);
        assert_eq!(e.query(&query("AB-123 C++", "content")).unwrap().total, 1);
        assert_eq!(e.query(&query("AB-999 C++", "content")).unwrap().total, 0);
        assert_eq!(
            e.query(&query("合同.txt", "name")).unwrap().entries[0].name,
            "合同.txt"
        );
        assert_eq!(e.query(&query("   ", "content")).unwrap().total, 0);
        assert!(e.query(&query("\" OR *", "content")).is_ok());
        // Missing word indexes stay pending while paused, keeping literal hits.
        e.content_action(&scope, "pause").unwrap();
        e.lock()
            .unwrap()
            .execute("DELETE FROM content_words", [])
            .unwrap();
        assert_eq!(e.content_status(&scope).unwrap().scopes[0].pending, 3);
        assert!(e.next_content_job().unwrap().is_none());
        assert_eq!(hits(&e, "付款", "content", &scope).total, 2);
        e.content_action(&scope, "resume").unwrap();
        index(&e);
        assert_eq!(e.query(&q).unwrap().total, 2);
        assert_eq!(e.content_status(&scope).unwrap().scopes[0].pending, 0);
        // Current file membership and disabled scopes still gate all word hits.
        e.content_action(&scope, "disable").unwrap();
        assert_eq!(e.query(&q).unwrap().total, 0);
        assert_eq!(
            e.lock()
                .unwrap()
                .query_row("SELECT count(*) FROM content_words", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(e.query(&query("合同 最终版", "name")).unwrap().total, 1);
    }

    #[test]
    fn large_document_tail_snippets_and_previews_are_searchable_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let text = format!("{}合同末尾 EndMarker", "正文🙂".repeat(320_000));
        fs::write(root.join("large.txt"), &text).unwrap();
        let e = Engine::open(temp.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, true)).unwrap();
        index(&e);
        for search in ["合同末尾", "合同", "末", "endmarker", "合同末尾 EndMarker"] {
            let result = hits(&e, search, "content", &scope);
            assert_eq!(result.total, 1, "{search}");
            let item = &result.entries[0];
            assert_eq!(
                item.snippet.as_ref().unwrap().matched.to_ascii_lowercase(),
                search.to_ascii_lowercase()
            );
            let preview = e.content_preview(&item.id, search).unwrap();
            assert!(preview.text.contains("合同末尾 EndMarker"));
            assert!(preview.text.len() <= 65536);
            assert!(preview.truncated);
        }
        assert_eq!(e.content_status(&scope).unwrap().scopes[0].truncated, 0);
        assert_eq!(hits(&e, "合同末尾", "name", &scope).total, 0);
        assert_eq!(hits(&e, "EndMarker 合同末尾", "content", &scope).total, 0);
    }

    #[test]
    fn old_truncated_rows_upgrade_without_losing_search_or_retrying_failures() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("large.txt"),
            format!("oldprefix {}newtail", "正文".repeat(360_000)),
        )
        .unwrap();
        fs::write(root.join("small.txt"), "complete-document").unwrap();
        let e = Engine::open(temp.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, true)).unwrap();
        index(&e);
        let id = hits(&e, "newtail", "content", &scope).entries[0].id.clone();
        {
            let c = e.lock().unwrap();
            c.execute(
                "UPDATE content_items SET text='oldprefix',truncated=1 WHERE entry_id=?",
                [&id],
            )
            .unwrap();
            c.execute_batch("ALTER TABLE content_items DROP COLUMN text_limit;")
                .unwrap();
            migrate(&c).unwrap();
        }
        e.content_action(&scope, "pause").unwrap();
        assert!(e.next_content_job().unwrap().is_none());
        assert_eq!(hits(&e, "oldprefix", "content", &scope).total, 1);
        let status = e.content_status(&scope).unwrap();
        assert_eq!((status.scopes[0].ready, status.scopes[0].pending), (1, 1));
        e.content_action(&scope, "resume").unwrap();
        let jobs = e.next_content_jobs_after(0, 128).unwrap();
        assert_eq!(jobs.len(), 1);
        let mut prepared = e.prepare_content_job(&jobs[0], None).unwrap().unwrap();
        prepared.output = Err(anyhow::anyhow!("synthetic parser failure"));
        e.commit_content_jobs(vec![prepared]).unwrap();
        assert_eq!(hits(&e, "oldprefix", "content", &scope).total, 1);
        // Expansion failure must not retry parsing. One cached-prefix word
        // backfill is still needed after migrating an old truncated document.
        let word_job = e.next_content_job().unwrap().unwrap();
        let prepared = e.prepare_content_job(&word_job, None).unwrap().unwrap();
        assert!(prepared.words_only);
        assert_eq!(prepared.output.as_ref().unwrap().text, "oldprefix");
        e.commit_content_jobs(vec![prepared]).unwrap();
        assert!(e.next_content_job().unwrap().is_none());
        assert!(e.content_status(&scope).unwrap().issues[0]
            .message
            .contains("保留原有部分内容"));
        // Simulate the old cap again, then verify successful automatic expansion.
        e.lock()
            .unwrap()
            .execute(
                "UPDATE content_items SET text_limit=2097152 WHERE entry_id=?",
                [&id],
            )
            .unwrap();
        index(&e);
        assert_eq!(hits(&e, "newtail", "content", &scope).total, 1);
        let status = e.content_status(&scope).unwrap();
        assert_eq!(
            (
                status.scopes[0].ready,
                status.scopes[0].pending,
                status.scopes[0].truncated
            ),
            (2, 0, 0)
        );
        assert!(e.next_content_job().unwrap().is_none());
        e.lock()
            .unwrap()
            .execute(
                "INSERT INTO content_fts(content_fts,rank) VALUES('integrity-check',1)",
                [],
            )
            .unwrap();
    }

    #[test]
    fn pending_text_budget_blocks_until_a_result_is_released() {
        let budget = Arc::new(ContentBudget::default());
        let first = budget.reserve(CONTENT_BUFFER_BUDGET / 2);
        let second = budget.reserve(CONTENT_BUFFER_BUDGET / 2);
        let (sender, receiver) = std::sync::mpsc::channel();
        let other = budget.clone();
        let worker = thread::spawn(move || {
            let permit = other.reserve(1);
            sender.send(()).unwrap();
            drop(permit);
        });
        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first);
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        drop(second);
        assert_eq!(*budget.used.lock().unwrap(), 0);
    }
    #[test]
    fn batched_jobs_choose_one_owner_and_revalidate_before_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        fs::create_dir_all(root.join("nested")).unwrap();
        for i in 0..12 {
            fs::write(
                root.join(format!("nested/{i}.txt")),
                "original indexed text",
            )
            .unwrap();
        }
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        let _parent = e.add_scope(input(&root, true)).unwrap();
        let child = e.add_scope(input(&root.join("nested"), true)).unwrap();
        wait_scan(&e);
        let jobs = e.next_content_jobs_after(0, 128).unwrap();
        assert_eq!(jobs.len(), 12);
        assert!(jobs.iter().all(|j| j.scope == child));
        let mut prepared = Vec::new();
        for job in &jobs {
            prepared.push(e.prepare_content_job(job, None).unwrap().unwrap());
        }
        // Revoking a scope after parsing must prevent every buffered result
        // from being committed, not just the currently active parser's result.
        e.content_action(&child, "pause").unwrap();
        e.commit_content_jobs(prepared).unwrap();
        assert_eq!(hits(&e, "original", "content", "").total, 0);
        let jobs = e.next_content_jobs_after(0, 128).unwrap();
        assert_eq!(jobs.len(), 12);
        let prepared = jobs
            .iter()
            .filter_map(|j| e.prepare_content_job(j, None).unwrap())
            .collect();
        e.commit_content_jobs(prepared).unwrap();
        assert_eq!(hits(&e, "original", "content", "").total, 12);
        assert!(e.next_content_jobs_after(0, 128).unwrap().is_empty());
    }

    #[test]
    fn opt_in_chinese_short_phrases_literal_symbols_and_scope_filtering() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("files");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("a.txt"),
            "合同签订后付款。 RDMA 网络加速。 literal \"quote\" and OR * 100%_ok",
        )
        .unwrap();
        fs::write(root.join("RDMA-name.txt"), "unrelated body").unwrap();
        let e = Engine::open(dir.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, false)).unwrap();
        index(&e);
        assert_eq!(hits(&e, "合同", "content", "").total, 0);
        e.content_action(&scope, "enable").unwrap();
        index(&e);
        for term in [
            "合",
            "合同",
            "合同签订",
            "rdma",
            "RDMA 网络",
            "\"quote\"",
            "100%_ok",
            "OR *",
        ] {
            let r = hits(&e, term, "content", &scope);
            assert_eq!(r.total, 1, "{term}");
            assert!(r.entries[0].snippet.is_some());
        }
        assert_eq!(hits(&e, "RDMA", "name", "").total, 1);
        assert_eq!(hits(&e, "RDMA", "all", "").total, 2);
        assert_eq!(hits(&e, "不存在", "content", "").total, 0);
        let id = hits(&e, "合同", "content", "").entries[0].id.clone();
        e.set_hidden(std::slice::from_ref(&id), true).unwrap();
        assert_eq!(hits(&e, "合同", "content", "").total, 0);
        assert_eq!(
            e.query(&Query {
                search: "合同".into(),
                search_mode: "content".into(),
                hidden: true,
                ..Default::default()
            })
            .unwrap()
            .total,
            1
        );
        e.set_hidden(&[id], false).unwrap();
        let disabled = e.add_scope(input(&root.join("nested"), false));
        assert!(disabled.is_err());
        e.content_action(&scope, "disable").unwrap();
        assert_eq!(hits(&e, "合同", "content", "").total, 0);
        assert_eq!(
            e.lock()
                .unwrap()
                .query_row("SELECT count(*) FROM content_items", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn directory_hiding_filters_content_and_combined_search() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("files");
        fs::create_dir_all(root.join("hidden")).unwrap();
        fs::write(root.join("hidden/合同.txt"), "保密项目 alpha").unwrap();
        fs::write(root.join("visible.txt"), "保密项目 alpha").unwrap();
        let e = Engine::open(dir.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, true)).unwrap();
        index(&e);
        e.hide_directory(&root.join("hidden")).unwrap();
        for mode in ["content", "all"] {
            assert_eq!(hits(&e, "保密项目", mode, &scope).total, 1);
            let result = e
                .query(&Query {
                    search: "alpha".into(),
                    search_mode: mode.into(),
                    hidden: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(result.total, 1);
            assert_eq!(result.entries[0].name, "合同.txt");
            assert!(result.entries[0].snippet.is_some());
        }
        let rule = e.summary().unwrap().hidden_directories.remove(0);
        e.restore_directory(&rule.id).unwrap();
        assert_eq!(hits(&e, "保密项目", "content", &scope).total, 2);
    }
    #[test]
    fn edits_moves_rebuilds_and_removal_do_not_leave_stale_fts_hits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("files");
        let target = dir.path().join("target");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(root.join("a.txt"), "oldtoken 合同").unwrap();
        let e = Engine::open(dir.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, true)).unwrap();
        let dest = e.add_scope(input(&target, true)).unwrap();
        index(&e);
        fs::write(root.join("a.txt"), "newtoken 新内容 changed length").unwrap();
        e.refresh(&scope).unwrap();
        wait_scan(&e);
        assert_eq!(hits(&e, "oldtoken", "content", "").total, 0);
        index(&e);
        assert_eq!(hits(&e, "newtoken", "content", "").total, 1);
        let id = hits(&e, "newtoken", "content", "").entries[0].id.clone();
        let plan = e
            .preview_move(std::slice::from_ref(&id), &target.to_string_lossy())
            .unwrap();
        e.execute_move(&plan.operation.id).unwrap();
        let start = Instant::now();
        while e.move_plan(&plan.operation.id).unwrap().operation.state == "running" {
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(20));
        }
        index(&e);
        assert_eq!(hits(&e, "newtoken", "content", &scope).total, 0);
        let r = hits(&e, "newtoken", "content", &dest);
        assert_eq!(r.entries[0].id, id);
        e.content_action(&dest, "pause").unwrap();
        assert!(e.next_content_job().unwrap().is_none());
        e.content_action(&dest, "rebuild").unwrap();
        assert_eq!(hits(&e, "newtoken", "content", "").total, 0);
        index(&e);
        fs::remove_file(target.join("a.txt")).unwrap();
        e.refresh(&dest).unwrap();
        wait_scan(&e);
        assert_eq!(hits(&e, "newtoken", "content", "").total, 0);
        e.lock()
            .unwrap()
            .execute(
                "INSERT INTO content_fts(content_fts,rank) VALUES('integrity-check',1)",
                [],
            )
            .unwrap();
        e.remove_scope(&scope).unwrap();
        e.remove_scope(&dest).unwrap();
        assert_eq!(e.content_status("").unwrap().scopes.len(), 0);
    }
    #[test]
    fn paused_or_revoked_job_cannot_commit_and_excluded_child_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("files");
        fs::create_dir_all(root.join("private")).unwrap();
        fs::write(root.join("public.txt"), "shared keyword").unwrap();
        fs::write(root.join("private/a.txt"), "secretword").unwrap();
        let e = Engine::open(dir.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, true)).unwrap();
        wait_scan(&e);
        let job = e.next_content_job().unwrap().unwrap();
        e.content_action(&scope, "pause").unwrap();
        e.index_content_job(&job).unwrap();
        assert_eq!(hits(&e, "shared", "content", "").total, 0);
        let mut settings = input(&root, true);
        settings.excludes = vec!["private".into()];
        e.update_scope(&scope, settings).unwrap();
        e.content_action(&scope, "resume").unwrap();
        index(&e);
        assert_eq!(hits(&e, "secretword", "content", "").total, 0);
        assert_eq!(hits(&e, "shared", "content", "").total, 1);
        let nested = e.add_scope(input(&root.join("private"), false)).unwrap();
        wait_scan(&e);
        assert_eq!(hits(&e, "secretword", "content", &nested).total, 0);
        e.content_action(&scope, "rebuild").unwrap();
        let job = e.next_content_job().unwrap().unwrap();
        e.remove_scope(&scope).unwrap();
        assert!(!e.valid_content_job(&job));
    }
    #[test]
    fn failures_and_unsupported_formats_are_visible_and_do_not_loop() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("files");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("broken.pdf"), "invalid PDF").unwrap();
        fs::write(root.join("empty.txt"), "").unwrap();
        fs::write(root.join("photo.png"), [0u8; 10]).unwrap();
        let e = Engine::open(dir.path().join("db/index.sqlite")).unwrap();
        let scope = e.add_scope(input(&root, true)).unwrap();
        index(&e);
        let status = e.content_status(&scope).unwrap();
        let s = &status.scopes[0];
        assert_eq!(
            (s.eligible, s.failed, s.skipped, s.unsupported, s.pending),
            (2, 1, 1, 1, 0)
        );
        assert_eq!(status.issues.len(), 2);
        assert!(e.next_content_job().unwrap().is_none());
    }
    #[test]
    fn v3_index_upgrades_with_content_disabled_and_fts_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let c = Connection::open(&path).unwrap();
        c.execute_batch("PRAGMA user_version=3;").unwrap();
        drop(c);
        let c = db::connect(&path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
        c.execute(
            "INSERT INTO content_fts(content_fts) VALUES('integrity-check')",
            [],
        )
        .unwrap();
    }
}
