pub mod content;
mod db;
mod deletion;
mod everything;
mod extract;
mod hidden;
pub mod model;
mod moving;
pub fn run_helper_if_requested() -> bool {
    extract::run_helper_if_requested() || everything::run_helper_if_requested()
}
mod performance;
mod platform;
mod rules;
mod scan;
mod scan_entry;
mod scan_gate;
mod scan_issues;
mod scan_progress;
mod scan_schedule;
mod watch;
#[cfg(windows)]
mod windows_shell;

use anyhow::{anyhow, bail, Context, Result};
use model::*;
use notify::{RecursiveMode, Watcher};
use platform::*;
use rusqlite::{params, Connection};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, SyncSender},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}
struct Inner {
    db: Mutex<Connection>,
    query_db: Mutex<Connection>,
    content_db: Mutex<Connection>,
    status_db: Mutex<Connection>,
    summary_cache: Mutex<Option<Summary>>,
    // Keep the OS lock until SQLite and every background worker have closed.
    _index_lock: fs::File,
    storage: PathBuf,
    controls: Mutex<HashMap<String, Arc<Control>>>,
    watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
    pool: Mutex<Arc<rayon::ThreadPool>>,
    file_operations: Mutex<()>,
    everything_query: Mutex<()>,
    scan_gate: scan_gate::ScanGate,
    scan_scheduler: scan_schedule::Scheduler,
    scan_runs: Mutex<scan_progress::Registry>,
    scan_requests: Mutex<()>,
}
pub(crate) struct Control {
    sender: SyncSender<()>,
    cancel: AtomicBool,
    removed: AtomicBool,
    dirty: AtomicBool,
    watch_failed: AtomicBool,
    pending: Mutex<watch::Pending>,
    progress: Mutex<Option<(std::time::Instant, ScanProgress)>>,
}
impl Engine {
    pub fn dispatch(&self, command: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        use serde_json::{json, to_value};
        let id = || {
            args.get("id")
                .and_then(|v| v.as_str())
                .context("缺少文件或文件夹 ID")
        };
        match command {
            "scan_issues" => Ok(to_value(
                self.scan_issues(
                    id()?,
                    args.get("offset")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                        .min(usize::MAX as u64) as usize,
                    args.get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(50)
                        .min(100) as usize,
                )?,
            )?),
            "content_status" => Ok(to_value(
                self.content_status(args.get("scopeId").and_then(|v| v.as_str()).unwrap_or(""))?,
            )?),
            "content_action" => {
                self.content_action(
                    id()?,
                    args.get("action")
                        .and_then(|v| v.as_str())
                        .context("缺少内容索引操作")?,
                )?;
                Ok(json!(null))
            }
            "content_preview" => Ok(to_value(self.content_preview(
                id()?,
                args.get("search").and_then(|v| v.as_str()).unwrap_or(""),
            )?)?),
            "everything_status" => Ok(to_value(self.everything_status())?),
            "set_everything" => Ok(to_value(
                self.set_everything(
                    args.get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                )?,
            )?),
            "summary" => {
                let summary = self.summary_for(
                    args.get("scopeId").and_then(|v| v.as_str()).unwrap_or(""),
                    args.get("hidden")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                )?;
                Ok(to_value(summary)?)
            }
            "performance" => Ok(to_value(self.performance()?)?),
            "query" => Ok(to_value(self.query(&serde_json::from_value(args)?)?)?),
            "add_scope" => Ok(json!({"id":self.add_scope(serde_json::from_value(args)?)?})),
            "update_scope" => {
                self.update_scope(
                    id()?,
                    serde_json::from_value(args.get("input").context("缺少文件夹配置")?.clone())?,
                )?;
                Ok(json!(null))
            }
            "remove_scope" => {
                self.remove_scope(id()?)?;
                Ok(json!(null))
            }
            "refresh" => {
                if let Some(id) = args.get("id").and_then(|v| v.as_str()) {
                    self.refresh(id)?;
                } else {
                    self.refresh_many(
                        &self
                            .summary()?
                            .scopes
                            .iter()
                            .map(|s| s.id.clone())
                            .collect::<Vec<_>>(),
                    )?;
                }
                Ok(json!(null))
            }
            "cancel" => {
                self.cancel(id()?)?;
                Ok(json!(null))
            }
            "hide_directory" => {
                self.hide_directory(Path::new(
                    args.get("path")
                        .and_then(|v| v.as_str())
                        .context("缺少目录路径")?,
                ))?;
                Ok(json!(null))
            }
            "restore_directory" => {
                self.restore_directory(id()?)?;
                Ok(json!(null))
            }
            "set_hidden" => {
                let ids: Vec<String> =
                    serde_json::from_value(args.get("ids").context("缺少文件选择")?.clone())?;
                self.set_hidden(
                    &ids,
                    args.get("hidden")
                        .and_then(|v| v.as_bool())
                        .context("缺少隐藏状态")?,
                )?;
                Ok(json!(null))
            }
            "preview_text" => Ok(to_value(self.preview_text(id()?)?)?),
            "open_entry" => {
                self.open_entry(
                    id()?,
                    args.get("reveal")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                )?;
                Ok(json!(null))
            }
            "add_demo" => Ok(json!({"id":self.add_demo()?})),
            "preview_move" => Ok(to_value(
                self.preview_move(
                    &serde_json::from_value::<Vec<String>>(
                        args.get("ids").context("缺少文件选择")?.clone(),
                    )?,
                    args.get("destination")
                        .and_then(|v| v.as_str())
                        .context("请选择目标文件夹")?,
                )?,
            )?),
            "execute_move" => Ok(to_value(self.execute_move(id()?)?)?),
            "move_plan" => Ok(to_value(self.move_plan(id()?)?)?),
            "move_history" => Ok(to_value(self.move_history()?)?),
            "cancel_move" => {
                self.cancel_move(id()?)?;
                Ok(json!(null))
            }
            "preview_delete" => Ok(to_value(self.preview_delete(
                &serde_json::from_value::<Vec<String>>(
                    args.get("ids").context("缺少文件选择")?.clone(),
                )?,
            )?)?),
            "execute_delete" => Ok(to_value(self.execute_delete(id()?)?)?),
            "cancel_delete" => {
                self.cancel_delete(id()?)?;
                Ok(json!(null))
            }
            "delete_plan" => Ok(to_value(self.delete_plan(id()?)?)?),
            "delete_history" => Ok(to_value(self.delete_history()?)?),
            _ => bail!("未知命令：{command}"),
        }
    }
    fn add_demo(&self) -> Result<String> {
        use std::io::Write;
        let dir = self
            .inner
            .storage
            .parent()
            .context("无法定位示例目录")?
            .join("示例资料");
        let files=[
        ("文档/项目说明.md","# FileM 示例资料\n\n这是供你体验文件聚合的独立示例文件夹。\n\n- 文件按类型聚合，原位置保持不变。\n- 点击后缀可查看不同目录中的同类文件。\n- 隐藏只改变视图，可从“已隐藏”恢复。\n\n你也可以点击“添加文件夹”选择自己的文件夹。\n"),
        ("设计/配色方案.md","# 配色方案\n\n主色：#29675B\n选中背景：#E8F0ED\n正文：#24343B\n侧栏：#F5F7F6\n\n清晰的目录信息优先于装饰。\n"),
        ("文档/阅读清单.txt","FileM 技术阅读\n1. Rust 文件系统与原生路径\n2. SQLite 批量事务\n3. ReadDirectoryChangesW 的溢出恢复\n4. Tauri 进程间通信\n"),
        ("文档/notes.md","# 使用笔记\n\n相同名称不等于相同内容。\n修改时间只能提供线索，不能自动决定文件版本。\n"),
        ("代码/index.ts","export const filem = {\n  name: 'FileM',\n  scope: '用户明确选择的文件夹',\n  indexing: 'metadata-only',\n};\n"),
        ("代码/package.json","{\n  \"name\": \"filem-example\",\n  \"private\": true\n}\n"),
        ("设计/palette.svg","<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"600\" height=\"240\"><rect width=\"200\" height=\"240\" fill=\"#29675b\"/><rect x=\"200\" width=\"200\" height=\"240\" fill=\"#e8f0ed\"/><rect x=\"400\" width=\"200\" height=\"240\" fill=\"#f5f7f6\"/></svg>"),
      ];
        for (name, content) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap())?;
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut f) => f.write_all(content.as_bytes())?,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(scope) = self
            .summary()?
            .scopes
            .into_iter()
            .find(|s| s.path == display_path(&dir))
        {
            self.refresh(&scope.id)?;
            return Ok(scope.id);
        }
        self.add_scope(ScopeInput {
            content_enabled: false,
            path: dir.to_string_lossy().into(),
            recursive: true,
            watch: true,
            excludes: DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect(),
        })
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let storage = path.parent().unwrap_or(Path::new(".")).canonicalize()?;
        let index_lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(storage.join(".filem-index.lock"))?;
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        loop {
            match index_lock.try_lock() {
                Ok(()) => break,
                Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(fs::TryLockError::WouldBlock) => {
                    bail!("这个索引已由另一个 FileM 实例打开，请先关闭原来的实例。");
                }
                Err(fs::TryLockError::Error(error)) => return Err(error).context("无法锁定索引"),
            }
        }
        let engine = Self {
            inner: Arc::new(Inner {
                db: Mutex::new(db::connect(path)?),
                query_db: Mutex::new(db::connect_reader(path)?),
                content_db: Mutex::new(db::connect_reader(path)?),
                status_db: Mutex::new(db::connect_reader(path)?),
                summary_cache: Mutex::new(None),
                _index_lock: index_lock,
                storage,
                controls: Mutex::new(HashMap::new()),
                watchers: Mutex::new(HashMap::new()),
                pool: Mutex::new(Arc::new(
                    rayon::ThreadPoolBuilder::new().num_threads(1).build()?,
                )),
                file_operations: Mutex::new(()),
                everything_query: Mutex::new(()),
                scan_gate: scan_gate::ScanGate::default(),
                scan_scheduler: scan_schedule::Scheduler::default(),
                scan_runs: Mutex::new(scan_progress::Registry::default()),
                scan_requests: Mutex::new(()),
            }),
        };
        engine.configure_performance()?;
        engine.start_content_worker();
        let scopes = engine.summary()?.scopes;
        for scope in &scopes {
            engine.register(&scope.id)?;
        }
        engine.refresh_many(&scopes.iter().map(|s| s.id.clone()).collect::<Vec<_>>())?;
        Ok(engine)
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.inner
            .db
            .lock()
            .map_err(|_| anyhow!("索引状态不可用，请重新启动应用。"))
    }
    pub fn summary(&self) -> Result<Summary> {
        self.summary_for("", false)
    }
    fn status_reader(&self) -> Result<MutexGuard<'_, Connection>> {
        self.inner
            .status_db
            .lock()
            .map_err(|_| anyhow!("索引状态不可用，请重新启动应用。"))
    }
    fn summary_for(&self, scope_id: &str, hidden: bool) -> Result<Summary> {
        let c = self.status_reader()?;
        let tx = c.unchecked_transaction()?;
        let revision = db::revision(&tx)?;
        let mut cache = self.inner.summary_cache.lock().unwrap();
        let mut summary = match cache.as_ref() {
            Some(previous)
                if previous.revision == revision
                    && previous.facet_scope_id == scope_id
                    && previous.facet_hidden == hidden =>
            {
                previous.clone()
            }
            _ => {
                let next = db::summary_for(&tx, scope_id, hidden)?;
                *cache = Some(next.clone());
                next
            }
        };
        summary.scan_paused = self.inner.scan_gate.is_paused();
        summary.scan_runs = self.inner.scan_runs.lock().unwrap().snapshot();
        let controls = self.inner.controls.lock().unwrap();
        for scope in &mut summary.scopes {
            scope.progress = controls.get(&scope.id).and_then(|control| {
                control
                    .progress
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|(started, progress)| {
                        let mut progress = progress.clone();
                        progress.elapsed_ms = started.elapsed().as_millis() as u64;
                        progress
                    })
            });
        }
        Ok(summary)
    }
    fn query_local(&self, q: &Query) -> Result<QueryResult> {
        let c = self
            .inner
            .query_db
            .lock()
            .map_err(|_| anyhow!("索引状态不可用，请重新启动应用。"))?;
        // Count, page and revision must describe the same committed WAL snapshot.
        let tx = c.unchecked_transaction()?;
        db::query(&tx, q)
    }
    pub fn query(&self, q: &Query) -> Result<QueryResult> {
        if q.search.chars().count() > 512 || q.search.contains('\0') {
            bail!("搜索内容最多 512 个字符，且不能包含空字符。");
        }
        if !["", "name", "content", "all"].contains(&q.search_mode.as_str()) {
            bail!("未知搜索模式");
        }
        if q.search_mode == "content" || q.search_mode == "all" {
            self.query_local(q)
        } else {
            self.query_with_everything(q)
        }
    }
    pub fn add_scope(&self, input: ScopeInput) -> Result<String> {
        rules::Excludes::new(&input.excludes)?;
        let path = checked_root(Path::new(&input.path))?;
        if path.starts_with(&self.inner.storage) {
            bail!("不能把 FileM 自身的索引数据目录加入文件夹。");
        }
        let identity = identity(&path, &fs::metadata(&path)?)?;
        let id = uuid::Uuid::new_v4().to_string();
        {
            let c = self.lock()?;
            let count: i64 = c.query_row("SELECT COUNT(*) FROM scopes", [], |r| r.get(0))?;
            if count >= 32 {
                bail!("当前验证版最多支持 32 个监控文件夹。");
            }
            let duplicate: i64 = c.query_row(
                "SELECT COUNT(*) FROM scopes WHERE root=? OR identity=?",
                params![encode(path.as_os_str()), identity],
                |r| r.get(0),
            )?;
            if duplicate > 0 {
                bail!("这个文件夹已经在监控文件夹中。");
            }
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .to_string();
            c.execute("INSERT INTO scopes(id,root,path,name,identity,depth,recursive,watch,excludes,content_enabled) VALUES(?,?,?,?,?,?,?,?,?,?)",params![id,encode(path.as_os_str()),path.to_string_lossy(),name,identity,path.components().count() as i64,input.recursive,input.watch,serde_json::to_string(&input.excludes)?,input.content_enabled])?;
            db::bump(&c)?;
        }
        self.register(&id)?;
        self.refresh(&id)?;
        Ok(id)
    }
    pub fn update_scope(&self, id: &str, input: ScopeInput) -> Result<()> {
        rules::Excludes::new(&input.excludes)?;
        {
            let c = self.lock()?;
            let (stored_root, _, _, old) = db::root(&c, id)?;
            if old.path != input.path && checked_root(Path::new(&input.path))? != stored_root {
                bail!("重新定位请移除旧文件夹后，选择新的根目录。");
            }
            c.execute("DELETE FROM content_items WHERE entry_id IN (SELECT entry_id FROM memberships WHERE scope_id=?)", [id])?;
            c.execute("UPDATE scopes SET recursive=?,watch=?,excludes=?,content_enabled=?,content_epoch=content_epoch+1,config_version=config_version+1 WHERE id=?",params![input.recursive,input.watch,serde_json::to_string(&input.excludes)?,input.content_enabled,id])?;
            db::bump(&c)?;
        }
        self.configure_watch(id)?;
        self.refresh(id)
    }
    pub fn remove_scope(&self, id: &str) -> Result<()> {
        if let Some(c) = self.inner.controls.lock().unwrap().remove(id) {
            c.removed.store(true, Ordering::SeqCst);
            c.cancel.store(true, Ordering::SeqCst);
            let _ = c.sender.try_send(());
        }
        self.inner.watchers.lock().unwrap().remove(id);
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        tx.execute("DELETE FROM scopes WHERE id=?", [id])?;
        db::cleanup(&tx)?;
        db::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
    pub fn refresh(&self, id: &str) -> Result<()> {
        self.refresh_many(&[id.to_owned()])
    }
    fn refresh_many(&self, ids: &[String]) -> Result<()> {
        // Publish the entire request before any worker can claim it. Watcher
        // setup may take longer than the worker debounce interval on Windows.
        let _requests = self.inner.scan_requests.lock().unwrap();
        let controls = self.inner.controls.lock().unwrap().clone();
        let targets = ids
            .iter()
            .map(|id| {
                controls
                    .get(id)
                    .cloned()
                    .map(|control| (id, control))
                    .context("文件夹不存在")
            })
            .collect::<Result<Vec<_>>>()?;
        for (id, _) in &targets {
            self.configure_watch(id)?;
        }
        {
            let mut c = self.lock()?;
            let tx = c.transaction()?;
            for (id, _) in &targets {
                tx.execute("UPDATE scopes SET freshness='verifying' WHERE id=?", [id])?;
            }
            if !targets.is_empty() {
                db::bump(&tx)?;
            }
            tx.commit()?;
        }
        for (_, control) in &targets {
            control.cancel.store(false, Ordering::SeqCst);
            control.pending.lock().unwrap().rescan();
            control.dirty.store(true, Ordering::SeqCst);
        }
        for (_, control) in targets {
            let _ = control.sender.try_send(());
        }
        Ok(())
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        let controls = self.inner.controls.lock().unwrap();
        let control = controls.get(id).context("文件夹不存在")?;
        control.cancel.store(true, Ordering::SeqCst);
        let c = self.lock()?;
        db::set_state(
            &c,
            id,
            "available",
            "partial",
            Some("扫描已取消；点击刷新后恢复扫描与监听更新。"),
        )
    }
    fn register(&self, id: &str) -> Result<()> {
        let (sender, receiver) = sync_channel(1);
        let control = Arc::new(Control {
            sender,
            cancel: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            watch_failed: AtomicBool::new(false),
            pending: Mutex::new(watch::Pending::default()),
            progress: Mutex::new(None),
        });
        self.inner
            .controls
            .lock()
            .unwrap()
            .insert(id.into(), control.clone());
        let weak = Arc::downgrade(&self.inner);
        let weak_control = Arc::downgrade(&control);
        let id = id.to_string();
        thread::spawn(move || {
            while receiver.recv().is_ok() {
                let Some(control) = weak_control.upgrade() else {
                    break;
                };
                if control.removed.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
                while receiver.try_recv().is_ok() {}
                if control.cancel.load(Ordering::SeqCst) {
                    continue;
                }
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let engine = Engine { inner };
                let result = engine.scan_pending(&id, &control);
                if let Err(error) = result {
                    if let Ok(c) = engine.lock() {
                        let _ = db::set_state(
                            &c,
                            &id,
                            "available",
                            "partial",
                            Some(&error.to_string()),
                        );
                    }
                }
            }
        });
        Ok(())
    }
    fn configure_watch(&self, id: &str) -> Result<()> {
        self.inner.watchers.lock().unwrap().remove(id);
        let (root, _, _, input) = {
            let c = self.lock()?;
            db::root(&c, id)?
        };
        let control = self
            .inner
            .controls
            .lock()
            .unwrap()
            .get(id)
            .context("文件夹不存在")?
            .clone();
        if !input.watch {
            control.watch_failed.store(false, Ordering::SeqCst);
            return Ok(());
        }
        let storage = self.inner.storage.clone();
        let callback_control = control.clone();
        let rules = rules::Excludes::new(&input.excludes)?;
        let callback_root = root.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if callback_control.cancel.load(Ordering::SeqCst)
                || callback_control.removed.load(Ordering::SeqCst)
            {
                return;
            }
            let mut pending = callback_control.pending.lock().unwrap();
            match event {
                Ok(e) if e.need_rescan() || e.paths.is_empty() && !e.kind.is_access() => {
                    pending.rescan_because("文件监听丢失部分事件，需要完整核对")
                }
                Ok(e) if !e.kind.is_access() => {
                    pending.add(e.paths.into_iter().filter(|p| {
                        !p.starts_with(&storage)
                            && !rules.matches(p.strip_prefix(&callback_root).unwrap_or(p))
                    }));
                }
                Err(_) => pending.rescan_because("文件监听报错，需要完整核对"),
                _ => return,
            }
            if pending.full || !pending.paths.is_empty() {
                callback_control.dirty.store(true, Ordering::SeqCst);
                let _ = callback_control.sender.try_send(());
            }
        });
        match watcher.and_then(|mut w| {
            w.watch(
                &root,
                if input.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )?;
            Ok(w)
        }) {
            Ok(w) => {
                control.watch_failed.store(false, Ordering::SeqCst);
                self.inner.watchers.lock().unwrap().insert(id.into(), w);
            }
            Err(_) => {
                control.watch_failed.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }
    pub fn set_hidden(&self, ids: &[String], hidden: bool) -> Result<()> {
        if ids.len() > 10000 {
            bail!("单次最多处理 10000 个文件引用。");
        }
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        for id in ids {
            if !hidden && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM entries e JOIN directories d ON d.id=e.dir_id WHERE e.id=? AND d.hidden=1)",
                [id], |r| r.get::<_, bool>(0),
            )? {
                bail!("选择中有文件随目录隐藏，请在“已隐藏”中打开“管理目录规则”，先恢复对应目录。");
            }
            if tx.execute(
                "UPDATE entries SET hidden=? WHERE id=?",
                params![hidden, id],
            )? == 0
            {
                bail!("选择中有文件已经失效，请刷新后重新选择。");
            }
        }
        db::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
    fn checked_entry(&self, id: &str) -> Result<(PathBuf, String, u64, i64)> {
        // Snapshot only this entry's authorizations; never count the entire index here.
        // Release SQLite before touching disks or network shares.
        let (entry, roots) = {
            let c = self.lock()?;
            let entry = db::entry_path(&c, id)?;
            let mut statement =
                c.prepare_cached("SELECT scope_id FROM memberships WHERE entry_id=?")?;
            let scope_ids = statement
                .query_map([id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let roots = scope_ids
                .into_iter()
                .map(|scope_id| Ok((scope_id.clone(), db::root(&c, &scope_id)?)))
                .collect::<Result<Vec<_>>>()?;
            (entry, roots)
        };
        let (path, expected, size, time, attrs) = entry;
        let meta =
            fs::symlink_metadata(&path).context("文件不可访问，可能已移动或离线，请刷新索引。")?;
        if meta.file_type().is_symlink()
            || reparse(attributes(&meta))
            || placeholder(attrs | attributes(&meta))
        {
            bail!("此文件为链接或云占位文件，当前版本不读取其内容。");
        }
        if identity(&path, &meta)? != expected || meta.len() != size || mtime(&meta) != time {
            bail!("文件已发生变化，请刷新索引后重试。");
        }
        let canonical = path.canonicalize()?;
        let mut allowed = false;
        for (scope_id, (root, root_identity, version, input)) in roots {
            if let Ok(relative) = canonical.strip_prefix(&root) {
                if (input.recursive || relative.components().count() == 1)
                    && !rules::Excludes::new(&input.excludes)?.matches(relative)
                    && fs::metadata(&root)
                        .ok()
                        .and_then(|m| identity(&root, &m).ok())
                        .as_deref()
                        == Some(root_identity.as_str())
                {
                    // Settings/removal may have changed while filesystem checks ran.
                    allowed = self.lock()?.query_row(
                        "SELECT EXISTS(SELECT 1 FROM memberships m JOIN scopes s ON s.id=m.scope_id WHERE m.entry_id=? AND s.id=? AND s.config_version=?)",
                        params![id, scope_id, version], |r| r.get(0))?;
                    if allowed {
                        break;
                    }
                }
            }
        }
        if !allowed {
            bail!("文件不再位于有效的已授权文件夹中。");
        }
        Ok((canonical, expected, size, time))
    }
    pub fn preview_text(&self, id: &str) -> Result<TextPreview> {
        let (path, expected, size, time) = self.checked_entry(id)?;
        let ext = extension(&path.file_name().unwrap_or_default().to_string_lossy());
        if ![
            "txt", "md", "rs", "ts", "tsx", "js", "jsx", "json", "toml", "yaml", "yml", "csv",
            "log", "css", "html", "py", "go", "c", "cpp", "h", "sh",
        ]
        .contains(&ext.as_str())
        {
            bail!("当前版本支持文本预览，可通过系统应用打开此格式。");
        }
        let mut file = fs::File::open(&path)?;
        if file_identity(&file)? != expected {
            bail!("文件身份已改变，请刷新后重试。");
        }
        let mut bytes = Vec::new();
        file.by_ref().take(65537).read_to_end(&mut bytes)?;
        let opened_meta = file.metadata()?;
        if opened_meta.len() != size || mtime(&opened_meta) != time {
            bail!("读取期间文件发生变化，请稍后重试。");
        }
        let final_meta = fs::metadata(&path)?;
        if identity(&path, &final_meta)? != expected
            || final_meta.len() != size
            || mtime(&final_meta) != time
        {
            bail!("读取期间文件发生变化，请稍后重试。");
        }
        let truncated = bytes.len() > 65536;
        bytes.truncate(65536);
        Ok(TextPreview {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            truncated,
        })
    }
    pub fn open_entry(&self, id: &str, reveal: bool) -> Result<()> {
        let (path, _, _, _) = self.checked_entry(id)?;
        #[cfg(windows)]
        windows_shell::open(&path, reveal)?;
        #[cfg(not(windows))]
        if reveal {
            #[cfg(target_os = "macos")]
            {
                if !std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .status()?
                    .success()
                {
                    bail!("系统无法定位此文件，请刷新后重试。");
                }
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            open::that(path.parent().context("文件缺少父目录")?)?;
        } else {
            open::that(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn searches_and_status_read_committed_data_while_a_writer_is_busy() {
        let temp = tempfile::tempdir().unwrap();
        let engine = Engine::open(temp.path().join("index.sqlite")).unwrap();
        let c = engine.lock().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        tx.execute("UPDATE meta SET revision=100 WHERE id=1", [])
            .unwrap();
        let reader = engine.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let result = reader.query(&Query::default()).unwrap();
            let summary = reader.summary().unwrap();
            let content = reader.content_status("").unwrap();
            send.send((result.revision, summary.revision, content.scopes.len()))
                .unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(2));
        // Release the writer even on failure, so a regression cannot hang the suite.
        drop(tx);
        drop(c);
        worker.join().unwrap();
        assert_eq!(result.unwrap(), (0, 0, 0));
    }
    #[test]
    fn second_engine_cannot_reset_an_active_operation_and_lock_releases_on_close() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.sqlite");
        let first = Engine::open(&path).unwrap();
        first.lock().unwrap().execute("INSERT INTO operations(id,created,expires,state) VALUES('active',0,9999999999999,'running')", []).unwrap();
        let clone = first.clone();
        drop(first);
        assert!(Engine::open(&path).is_err());
        assert_eq!(clone.delete_plan("active").unwrap().state, "running");
        drop(clone);
        let reopened = Engine::open(&path).unwrap();
        assert_eq!(reopened.delete_plan("active").unwrap().state, "interrupted");
    }
}
