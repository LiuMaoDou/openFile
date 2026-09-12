//! Native CI smoke test. All files and the Everything configuration are disposable.
#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() -> anyhow::Result<()> {
    if filem_core::run_helper_if_requested() {
        return Ok(());
    }
    #[cfg(windows)]
    native::run()?;
    Ok(())
}

#[cfg(windows)]
mod native {
    use anyhow::{ensure, Context, Result};
    use filem_core::{
        model::{Query, ScopeInput},
        Engine,
    };
    use std::{
        fs,
        process::{Child, Command},
        thread,
        time::{Duration, Instant},
    };
    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    pub fn run() -> Result<()> {
        ensure!(
            std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
            "This smoke test requires an isolated GitHub Actions runner."
        );
        let executable = std::env::var("FILEM_EVERYTHING_EXE")
            .context("Set the verified Everything executable path")?;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("中文 资料");
        let target = temp.path().join("目标文件夹");
        let outside = temp.path().join("未授权文件夹");
        for path in [&source, &target, &outside] {
            fs::create_dir_all(path)?;
        }
        fs::create_dir_all(source.join("nested/private"))?;
        fs::write(source.join("报告 [draft].txt"), "source alpha")?;
        fs::write(source.join("other.md"), "source beta")?;
        fs::write(source.join("nested/private/secret.txt"), "excluded")?;
        fs::write(target.join("other.md"), "keep target")?;
        fs::write(outside.join("报告 [draft].txt"), "outside authorization")?;
        let engine = Engine::open(temp.path().join("state/index.sqlite"))?;
        ensure!(
            !engine.everything_status().available,
            "Runner already has an Everything instance; leave it untouched."
        );
        let ini = std::path::Path::new(&executable).with_file_name("Everything.ini");
        fs::write(&ini, format!("[Everything]\napp_data=0\nrun_as_admin=0\nshow_on_system_tray_double_click=0\ncheck_for_updates_on_startup=0\nauto_include_fixed_volumes=0\nauto_include_removable_volumes=0\nntfs_volume_includes=\nrefs_volume_includes=\nfolders={}\nfolder_monitor_changes=1\nfolder_update_types=0\nexclude_hidden_files_and_folders=0\nexclude_system_files_and_folders=0\netp_server_enabled=0\nhttp_server_enabled=0\n", temp.path().display()))?;
        let _everything = Process(
            Command::new(executable)
                .arg("-config")
                .arg(&ini)
                .arg("-db")
                .arg(temp.path().join("Everything.db"))
                .arg("-startup")
                .spawn()?,
        );
        let start = Instant::now();
        while !engine.everything_status().available {
            ensure!(
                start.elapsed() < Duration::from_secs(45),
                "Everything did not become ready"
            );
            thread::sleep(Duration::from_millis(300));
        }
        engine.set_everything(true)?;
        let input = |path: &std::path::Path| ScopeInput {
            path: path.to_string_lossy().into_owned(),
            recursive: true,
            watch: false,
            excludes: vec!["nested/private".into()],
        };
        let scope = engine.add_scope(input(&source))?;
        engine.add_scope(input(&target))?;
        let wait = || -> Result<()> {
            let start = Instant::now();
            while engine
                .summary()?
                .scopes
                .iter()
                .any(|s| s.freshness != "current")
            {
                ensure!(
                    start.elapsed() < Duration::from_secs(45),
                    "Native scan did not complete"
                );
                thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        };
        wait()?;
        println!(
            "Smoke source: {:?}, canonical: {:?}",
            source,
            source.canonicalize()
        );
        for search in ["", "[draft].txt"] {
            use std::os::windows::ffi::OsStrExt;
            use std::process::Stdio;
            let mut helper = Command::new(std::env::current_exe()?)
                .arg("--filem-everything-helper")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            let request = serde_json::json!({"roots":[source.as_os_str().encode_wide().collect::<Vec<_>>()],"search":search,"limit":50,"probe":false});
            serde_json::to_writer(helper.stdin.take().unwrap(), &request)?;
            let output = helper.wait_with_output()?;
            let reply: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            let paths = reply["paths"].as_array().map(|paths| {
                paths
                    .iter()
                    .map(|path| {
                        String::from_utf16_lossy(
                            &serde_json::from_value::<Vec<u16>>(path.clone()).unwrap(),
                        )
                    })
                    .collect::<Vec<_>>()
            });
            println!(
                "Raw SDK diagnostic search={search:?}: paths={paths:?}, error={:?}",
                reply["error"]
            );
        }
        let start = Instant::now();
        let found = loop {
            let result = engine.query(&Query {
                search: "[draft].txt".into(),
                scope_id: scope.clone(),
                ..Default::default()
            })?;
            if result.search_engine == "everything" && result.entries.len() == 1 {
                break result;
            }
            ensure!(
                start.elapsed() < Duration::from_secs(45),
                "Real SDK query did not find the fixture: {result:?}"
            );
            thread::sleep(Duration::from_millis(300));
        };
        ensure!(
            found.entries[0].name == "报告 [draft].txt",
            "Unicode/literal search mismatch"
        );
        ensure!(
            !found.entries[0].path.starts_with(r"\\?\"),
            "Verbatim prefix leaked into UI path"
        );
        ensure!(
            engine.preview_text(&found.entries[0].id)?.text == "source alpha",
            "Wrong SDK target"
        );
        let excluded = engine.query(&Query {
            search: "secret.txt".into(),
            scope_id: scope.clone(),
            ..Default::default()
        })?;
        ensure!(
            excluded.search_engine == "everything" && excluded.total == 0,
            "SDK bypassed nested exclusion"
        );
        let ids = engine
            .query(&Query {
                scope_id: scope,
                ..Default::default()
            })?
            .entries
            .into_iter()
            .map(|e| e.id)
            .collect::<Vec<_>>();
        let plan = engine.preview_move(&ids, &target.to_string_lossy())?;
        let moved = engine.execute_move(&plan.operation.id)?;
        ensure!(
            moved
                .operation
                .items
                .iter()
                .filter(|i| i.status == "succeeded")
                .count()
                == 1,
            "Native move failed: {moved:?}"
        );
        ensure!(
            fs::read_to_string(target.join("other.md"))? == "keep target",
            "Conflicting target overwritten"
        );
        ensure!(
            fs::read_to_string(source.join("other.md"))? == "source beta",
            "Skipped source changed"
        );
        wait()?;
        let id = found.entries[0].id.clone();
        ensure!(
            engine.preview_text(&id)?.text == "source alpha",
            "Moved identity or data lost"
        );
        let plan = engine.preview_delete(&[id])?;
        let deleted = engine.execute_delete(&plan.id)?;
        ensure!(
            deleted.items[0].status == "succeeded",
            "Native recycle failed: {deleted:?}"
        );
        ensure!(
            !target.join("报告 [draft].txt").exists(),
            "Recycled file still exists at source"
        );
        println!("PASS: real Everything IPC, UTF-16/literal/scoped search, exclusions, native batch move without overwrite, stable entry IDs, native recycle bin.");
        Ok(())
    }
}
