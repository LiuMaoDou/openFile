use crate::Engine;
use anyhow::{bail, Result};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Performance {
    pub mode: String,
    pub scan_threads: usize,
    pub content_threads: usize,
}
impl Performance {
    fn resolve(mode: &str) -> Result<Self> {
        let available = std::thread::available_parallelism().map_or(4, usize::from);
        let (scan_threads, content_threads) = match mode {
            "auto" => (available.clamp(1, 4), available.clamp(1, 2)),
            "low" => (1, 1),
            "balanced" => (4, 2),
            "fast" => (available.clamp(4, 16), available.clamp(2, 4)),
            _ => bail!("未知扫描性能模式"),
        };
        Ok(Self {
            mode: mode.into(),
            scan_threads,
            content_threads,
        })
    }
}
impl Engine {
    pub fn performance(&self) -> Result<Performance> {
        let c = self.inner.content_db.lock().unwrap();
        let mode: Option<String> = c
            .query_row(
                "SELECT value FROM settings WHERE key='performance'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Performance::resolve(mode.as_deref().unwrap_or("auto"))
    }
    pub fn set_performance(&self, mode: &str) -> Result<Performance> {
        let setting = Performance::resolve(mode)?;
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(setting.scan_threads)
                .build()?,
        );
        self.lock()?.execute("INSERT INTO settings(key,value) VALUES('performance',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[mode])?;
        *self.inner.pool.lock().unwrap() = pool;
        Ok(setting)
    }
    pub(crate) fn configure_performance(&self) -> Result<()> {
        let setting = self.performance()?;
        *self.inner.pool.lock().unwrap() = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(setting.scan_threads)
                .build()?,
        );
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_persist_and_invalid_modes_do_not_change_them() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.sqlite");
        let e = Engine::open(&path).unwrap();
        assert_eq!(e.set_performance("low").unwrap().scan_threads, 1);
        assert!(e.set_performance("unknown").is_err());
        assert_eq!(e.performance().unwrap().mode, "low");
        drop(e);
        let e = Engine::open(path).unwrap();
        assert_eq!(e.inner.pool.lock().unwrap().current_num_threads(), 1);
        let old = e.inner.pool.lock().unwrap().clone();
        assert_eq!(e.set_performance("balanced").unwrap().scan_threads, 4);
        assert_eq!(old.current_num_threads(), 1);
        assert_eq!(e.inner.pool.lock().unwrap().current_num_threads(), 4);
    }
}
