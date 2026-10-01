#![cfg_attr(windows, windows_subsystem = "windows")]
//! Uses only synthetic documents in an owned temporary directory.
use anyhow::{ensure, Result};
use filem_core::{
    model::{Query, ScopeInput},
    Engine,
};
use std::{
    fs,
    io::Write,
    thread,
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
    let large = format!(
        "{}跨块短语{}长文终点 LARGETAIL",
        "a".repeat(256 * 1024 - 2),
        "正文内容".repeat(200_000)
    );
    fs::write(root.join("large-utf8.txt"), &large)?;
    fs::write(
        root.join("large-gbk.txt"),
        encoding_rs::GBK.encode(&large).0,
    )?;
    fs::write(
        root.join("large-utf16.txt"),
        [
            vec![255, 254],
            large.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat(),
    )?;
    let mut docx = zip::ZipWriter::new(fs::File::create(root.join("large.docx"))?);
    docx.start_file(
        "word/document.xml",
        zip::write::SimpleFileOptions::default(),
    )?;
    write!(
        docx,
        "<w:document xmlns:w=\"w\"><w:p><w:r><w:t>{large}</w:t></w:r></w:p></w:document>"
    )?;
    docx.finish()?;
    let near_limit = "capacityboundary";
    let mut capped = "z".repeat(32 * 1024 * 1024 - near_limit.len());
    capped.push_str(near_limit);
    capped.push_str("beyondcapacity");
    fs::write(root.join("capacity.txt"), capped)?;
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
            if scope.eligible == 9 && scope.pending == 0 {
                for issue in &status.issues {
                    eprintln!("{}: {}", issue.name, issue.message);
                }
                ensure!(
                    scope.ready == 9 && scope.truncated == 1,
                    "not all documents fully indexed: ready={}, failed={}",
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
    for needle in ["长文终点", "largetail", "跨块短语", "跨块", "终"] {
        let results = e.query(&Query {
            search: needle.into(),
            ..q.clone()
        })?;
        ensure!(results.total == 4, "large-document search failed: {needle}");
        for item in &results.entries {
            ensure!(item.snippet.is_some(), "large-document snippet missing");
            let preview = e.content_preview(&item.id, needle)?;
            ensure!(
                preview
                    .text
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase()),
                "preview must include the match"
            );
            ensure!(preview.text.len() <= 65536, "preview must remain bounded");
        }
    }
    ensure!(
        e.query(&Query {
            search: near_limit.into(),
            ..q.clone()
        })?
        .total
            == 1,
        "32 MiB boundary must be searchable through the real parser"
    );
    ensure!(
        e.query(&Query {
            search: "beyondcapacity".into(),
            ..q.clone()
        })?
        .total
            == 0,
        "text beyond the advertised limit must remain marked as partial"
    );
    let keywords = Query {
        search: "largetail 跨块短语".into(),
        match_mode: "keywords".into(),
        sort: "relevance".into(),
        ..q.clone()
    };
    ensure!(
        e.query(&keywords)?.total == 4,
        "keyword search must combine distant terms in large documents"
    );
    e.content_action(&id, "disable")?;
    ensure!(
        e.query(&keywords)?.total == 0,
        "disabled word index must not return hits"
    );
    ensure!(
        e.query(&q)?.total == 0,
        "disabled content index must not return hits"
    );
    println!("PASS: isolated PDF/DOCX/XLSX/PPTX extraction; large UTF-8/GBK/UTF-16/DOCX tail and cross-frame search; 32 MiB boundary; keyword AND search; bounded previews; folder opt-in and cleanup.");
    Ok(())
}
