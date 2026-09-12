#![cfg_attr(windows, windows_subsystem = "windows")]
//! Uses only synthetic documents in an owned temporary directory.
use anyhow::{ensure, Result};
use filem_core::{
    model::{Query, ScopeInput},
    Engine,
};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
fn main() -> Result<()> {
    if filem_core::run_helper_if_requested() {
        return Ok(());
    }
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("中文内容测试");
    fs::create_dir(&root)?;
    for ext in ["pdf", "docx", "xlsx", "pptx"] {
        fs::copy(
            format!(
                "{}/tests/fixtures/content/sample.{ext}",
                env!("CARGO_MANIFEST_DIR")
            ),
            root.join(format!("资料.{ext}")),
        )?;
    }
    let e = Engine::open(temp.path().join("db/index.sqlite"))?;
    let id = e.add_scope(ScopeInput {
        path: root.to_string_lossy().into(),
        recursive: true,
        watch: false,
        excludes: vec![],
        content_enabled: true,
    })?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let status = e.content_status(&id)?;
        if let Some(scope) = status.scopes.first() {
            if scope.eligible == 4 && scope.pending == 0 {
                for issue in &status.issues {
                    eprintln!("{}: {}", issue.name, issue.message);
                }
                ensure!(
                    scope.ready == 4,
                    "not all four formats indexed: ready={}, failed={}",
                    scope.ready,
                    scope.failed
                );
                break;
            }
        }
        ensure!(Instant::now() < deadline, "content worker timed out");
        thread::sleep(Duration::from_millis(100));
    }
    let q = Query {
        search: "quartzneedle".into(),
        search_mode: "content".into(),
        ..Default::default()
    };
    ensure!(
        e.query(&q)?.total == 4,
        "full-text helper search must match all four formats"
    );
    ensure!(
        e.query(&Query {
            search: "合同".into(),
            ..q.clone()
        })?
        .total
            == 4,
        "Chinese phrase must match all formats"
    );
    ensure!(
        e.query(&Query {
            search: "quartzneedle".into(),
            search_mode: "name".into(),
            ..Default::default()
        })?
        .total
            == 0,
        "content must not leak into filename-only mode"
    );
    e.content_action(&id, "disable")?;
    ensure!(
        e.query(&q)?.total == 0,
        "disabled content index must not return hits"
    );
    println!("PASS: isolated native PDF/DOCX/XLSX/PPTX extraction, Chinese content search, folder opt-in and cleanup.");
    Ok(())
}
