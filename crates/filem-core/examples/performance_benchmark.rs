//! Reproducible, isolated synthetic scan/content benchmark. No user files touched.
use filem_core::{model::ScopeInput, Engine};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
fn main() {
    if filem_core::run_helper_if_requested() {
        return;
    }
    let content = std::env::var_os("FILEM_BENCH_CONTENT").is_some();
    let count: usize = std::env::var("FILEM_BENCH_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if content { 256 } else { 30000 });
    let directories: usize = std::env::var("FILEM_BENCH_DIRS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    assert!(directories > 0);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    fs::create_dir(&root).unwrap();
    for i in 0..directories {
        fs::create_dir(root.join(format!("dir-{i}"))).unwrap();
    }
    for i in 0..count {
        fs::write(
            root.join(format!("dir-{}/file-{i}.txt", i % directories)),
            format!("文件 {i} performance fixture\n"),
        )
        .unwrap();
    }
    let engine = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
    let start = Instant::now();
    let id = engine
        .add_scope(ScopeInput {
            path: root.to_string_lossy().into(),
            recursive: true,
            watch: false,
            excludes: vec![],
            content_enabled: content,
        })
        .unwrap();
    let mut scans = Vec::new();
    for round in 0..if content { 1 } else { 3 } {
        let timer = Instant::now();
        if round > 0 {
            engine.refresh(&id).unwrap();
        }
        loop {
            let summary = engine.summary().unwrap();
            assert!(summary.scopes.iter().all(|s| s.freshness != "partial"));
            if summary.scopes.iter().all(|s| s.freshness == "current") {
                break;
            }
            assert!(timer.elapsed() < Duration::from_secs(180));
            thread::sleep(Duration::from_millis(25));
        }
        scans.push(timer.elapsed().as_secs_f64() * 1000.);
    }
    let metadata_ms = start.elapsed().as_secs_f64() * 1000.;
    if content {
        loop {
            let status = engine.content_status("").unwrap();
            assert!(
                status
                    .scopes
                    .iter()
                    .all(|s| s.failed == 0 && s.skipped == 0),
                "content parsing failed"
            );
            if status.scopes.iter().map(|s| s.ready).sum::<u64>() == count as u64 {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(180));
            thread::sleep(Duration::from_millis(25));
        }
        let hits = engine
            .query(&filem_core::model::Query {
                search_mode: "content".into(),
                search: "performance".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.total, count as u64);
    }
    assert_eq!(engine.summary().unwrap().total, count as u64);
    println!(
        "{}",
        serde_json::json!({"platform":std::env::consts::OS,"files":count,"directories":directories+1,"content":content,"metadata_ms":metadata_ms,"scan_ms_including_debounce":scans,"total_ms":start.elapsed().as_secs_f64()*1000.,"note":"synthetic warm local fixtures; watch disabled; validates file count and content hits; not Windows acceptance"})
    );
}
