use crate::Engine;
use anyhow::Result;
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
    fn fast() -> Self {
        let available = std::thread::available_parallelism().map_or(4, usize::from);
        Self {
            mode: "fast".into(),
            scan_threads: available.clamp(4, 16),
            content_threads: available.clamp(2, 4),
        }
    }
}
impl Engine {
    pub fn performance(&self) -> Result<Performance> {
        // All scans use the same high-performance limits, including databases
        // that still contain a performance preference from an older version.
        Ok(Performance::fast())
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
    fn new_and_existing_indexes_always_use_high_performance() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.sqlite");
        let e = Engine::open(&path).unwrap();
        let setting = e.performance().unwrap();
        assert_eq!(setting.mode, "fast");
        assert!((4..=16).contains(&setting.scan_threads));
        assert!((2..=4).contains(&setting.content_threads));
        assert_eq!(
            e.inner.pool.lock().unwrap().current_num_threads(),
            setting.scan_threads
        );
        e.lock()
            .unwrap()
            .execute(
                "INSERT INTO settings(key,value) VALUES('performance','low')",
                [],
            )
            .unwrap();
        drop(e);
        let e = Engine::open(path).unwrap();
        assert_eq!(e.performance().unwrap().mode, "fast");
        assert_eq!(
            e.inner.pool.lock().unwrap().current_num_threads(),
            setting.scan_threads
        );
        assert!(e
            .dispatch("set_performance", serde_json::json!({"mode":"low"}))
            .is_err());
    }
}
