use filem_core::{
    model::{Query, ScopeInput},
    Engine,
};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
fn wait(engine: &Engine, id: &str) {
    let start = Instant::now();
    loop {
        let scope = engine
            .summary()
            .unwrap()
            .scopes
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        if scope.freshness == "current" {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(300),
            "scan timed out: {scope:?}"
        );
        thread::sleep(Duration::from_millis(100));
    }
}
fn main() {
    let count: usize = std::env::var("FILEM_BENCH_COUNT")
        .unwrap_or_else(|_| "10000".into())
        .parse()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("fixture");
    eprintln!("Creating {count} temporary files...");
    for folder in 0..100 {
        fs::create_dir_all(root.join(format!("folder-{folder:03}"))).unwrap();
    }
    for i in 0..count {
        let ext = ["md", "txt", "rs", "json"][i % 4];
        fs::write(
            root.join(format!("folder-{:03}/file-{i:07}.{ext}", i % 100)),
            format!("{{\"file\":{i}}}\n"),
        )
        .unwrap();
    }
    let engine = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
    eprintln!("Fixture ready. Scanning...");
    let start = Instant::now();
    let id = engine
        .add_scope(ScopeInput {
            path: root.to_string_lossy().into(),
            recursive: true,
            watch: false,
            excludes: vec![],
        })
        .unwrap();
    wait(&engine, &id);
    let mut scans = vec![start.elapsed().as_millis()];
    eprintln!("Scan 1/5: {} ms", scans[0]);
    for _ in 0..4 {
        let start = Instant::now();
        engine.refresh(&id).unwrap();
        wait(&engine, &id);
        scans.push(start.elapsed().as_millis());
        eprintln!("Scan {}/5: {} ms", scans.len(), scans.last().unwrap());
    }
    assert_eq!(engine.summary().unwrap().total, count as u64);
    let mut times = Vec::new();
    for i in 0..100 {
        let start = Instant::now();
        let result = engine
            .query(&Query {
                extension: "md".into(),
                sort: if i % 2 == 0 { "name" } else { "size" }.into(),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.total, count.div_ceil(4) as u64);
        times.push(start.elapsed().as_micros());
    }
    times.sort();
    println!(
        "{}",
        serde_json::json!({"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"files":count,"directories":100,"scan_ms_including_debounce":scans,"query_samples":100,"query_p50_us":times[49],"query_p95_us":times[94],"query_max_us":times[99],"note":"temporary metadata fixtures; backend only; warm repeated scans; not Windows or full UI measurements"})
    );
}
