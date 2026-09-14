use filem_core::{
    model::{Query, ScopeInput},
    Engine,
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn cpu_ms() -> f64 {
    #[cfg(unix)]
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
        (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
            + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0
    }
    #[cfg(not(unix))]
    {
        0.0
    }
}
fn rss_mib() -> f64 {
    #[cfg(unix)]
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
        #[cfg(target_os = "macos")]
        {
            usage.ru_maxrss as f64 / 1048576.0
        }
        #[cfg(not(target_os = "macos"))]
        {
            usage.ru_maxrss as f64 / 1024.0
        }
    }
    #[cfg(not(unix))]
    {
        0.0
    }
}
fn wait(engine: &Engine) {
    let start = Instant::now();
    loop {
        let summary = engine.summary().unwrap();
        if summary.scopes.iter().all(|s| s.freshness == "current") {
            return;
        }
        assert!(summary.scopes.iter().all(|s| s.freshness != "partial"));
        assert!(start.elapsed() < Duration::from_secs(120));
        thread::sleep(Duration::from_millis(100));
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(&args[1]);
    let mode = &args[2];
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::open(temp.path().join("index.sqlite")).unwrap();
    let roots = if mode == "single" {
        vec![root]
    } else {
        (0..4).map(|i| root.join(format!("group{i}"))).collect()
    };
    let mut ids = Vec::new();
    let mut scans = Vec::new();
    for run in 0..4 {
        let started = Instant::now();
        let cpu = cpu_ms();
        for (i, path) in roots.iter().enumerate() {
            if run == 0 {
                ids.push(
                    engine
                        .add_scope(ScopeInput {
                            path: path.to_string_lossy().into(),
                            recursive: true,
                            watch: false,
                            excludes: vec![],
                            content_enabled: false,
                        })
                        .unwrap(),
                );
            } else {
                engine.refresh(&ids[i]).unwrap();
            }
            if mode == "sequential" {
                wait(&engine);
            }
        }
        wait(&engine);
        let wall = started.elapsed().as_secs_f64() * 1000.0;
        let cpu = cpu_ms() - cpu;
        assert_eq!(engine.summary().unwrap().total, 100000);
        scans.push(
            serde_json::json!({"wall_ms":wall,"cpu_ms":cpu,"cpu_percent_one_core":100.0*cpu/wall}),
        );
    }
    assert_eq!(engine.query(&Query::default()).unwrap().total, 100000);
    println!(
        "{}",
        serde_json::json!({"mode":mode,"files":100000,"scopes":roots.len(),"threads":4,"scans":scans,"max_rss_mib":rss_mib()})
    );
}
