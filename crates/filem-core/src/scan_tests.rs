use super::*;
use std::{sync::mpsc, thread, time::Duration};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let engine = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        Self {
            _temp: temp,
            root,
            engine,
        }
    }
    fn file(&self, relative: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, relative).unwrap();
    }
    // Register valid scopes without background workers so races can be staged exactly.
    fn scope(&self, relative: &str, recursive: bool, excludes: &[&str]) -> (String, Arc<Control>) {
        let path = self.root.join(relative).canonicalize().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, _) = sync_channel(1);
        let control = Arc::new(Control {
            sender,
            cancel: false.into(),
            removed: false.into(),
            dirty: false.into(),
            watch_failed: false.into(),
            pending: Default::default(),
            progress: Default::default(),
        });
        control.pending.lock().unwrap().rescan();
        self.engine.lock().unwrap().execute(
            "INSERT INTO scopes(id,root,path,name,identity,depth,recursive,watch,excludes) VALUES(?,?,?,?,?,?,?,0,?)",
            params![id, encode(path.as_os_str()), display_path(&path), relative,
                identity(&path, &fs::metadata(&path).unwrap()).unwrap(), path.components().count() as i64,
                recursive, serde_json::to_string(excludes).unwrap()],
        ).unwrap();
        self.engine
            .inner
            .controls
            .lock()
            .unwrap()
            .insert(id.clone(), control.clone());
        (id, control)
    }
    fn names(&self, id: &str) -> Vec<String> {
        let c = self.engine.lock().unwrap();
        let mut s = c.prepare("SELECT e.name FROM entries e JOIN memberships m ON m.entry_id=e.id WHERE m.scope_id=? ORDER BY e.name").unwrap();
        s.query_map([id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
}

#[test]
fn overlapping_scopes_share_walk_but_keep_exclusions_and_recursion() {
    let f = Fixture::new();
    for name in [
        "a.txt",
        "child/b.txt",
        "child/c.tmp",
        "child/deep/d.txt",
        "child/deep/nested/e.txt",
        "child/skip/s.txt",
    ] {
        f.file(name);
    }
    let parent = f.scope("", true, &["child/skip", "*.tmp"]);
    let child = f.scope("child", true, &["deep"]);
    let flat = f.scope("child/deep", false, &[]);
    let targets = [parent.clone(), child.clone(), flat.clone()];
    let stats = run_group(&f.engine, &targets).unwrap();
    assert_eq!((stats.directories, stats.files), (5, 6));
    assert_eq!(f.names(&parent.0), ["a.txt", "b.txt", "d.txt", "e.txt"]);
    assert_eq!(f.names(&child.0), ["b.txt", "c.tmp", "s.txt"]);
    assert_eq!(f.names(&flat.0), ["d.txt"]);
    let summary = f.engine.summary().unwrap();
    assert_eq!(summary.total, 6);
    assert_eq!(summary.scan_runs.len(), 1);
    let run = &summary.scan_runs[0];
    assert_eq!(run.total_directories, Some(5));
    assert_eq!(run.processed_directories, 5);
    assert_eq!(run.checked_files, 6);
    assert_eq!(run.phase, "complete");
    for (id, control) in targets {
        let scope = summary.scopes.iter().find(|s| s.id == id).unwrap();
        assert_eq!(scope.freshness, "current");
        assert_eq!(scope.scanned as usize, f.names(&id).len());
        assert!(control.progress.lock().unwrap().is_none());
    }
}

#[test]
fn child_worker_claims_pending_parent_and_does_not_scan_twice() {
    let f = Fixture::new();
    f.file("a.txt");
    f.file("child/b.txt");
    let parent = f.scope("", true, &[]);
    let child = f.scope("child", true, &[]);
    f.engine.scan_pending(&child.0, &child.1).unwrap();
    assert_eq!(f.names(&parent.0), ["a.txt", "b.txt"]);
    assert_eq!(f.names(&child.0), ["b.txt"]);
    let generations: i64 = f
        .engine
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(DISTINCT generation) FROM memberships",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(generations, 1);
    let revision = f.engine.summary().unwrap().revision;
    f.engine.scan_pending(&parent.0, &parent.1).unwrap();
    assert_eq!(f.engine.summary().unwrap().revision, revision);
    assert!(!parent.1.pending.lock().unwrap().full);
    assert!(!child.1.pending.lock().unwrap().full);
}

#[test]
fn cancelling_one_member_keeps_other_members_running_and_old_memberships() {
    let f = Fixture::new();
    f.file("a.txt");
    f.file("child/b.txt");
    let parent = f.scope("", true, &[]);
    let child = f.scope("child", true, &[]);
    run_group(&f.engine, &[parent.clone(), child.clone()]).unwrap();
    fs::remove_file(f.root.join("child/b.txt")).unwrap();
    let pause = f.engine.inner.scan_gate.pause();
    let engine = f.engine.clone();
    let targets = [parent.clone(), child.clone()];
    let worker = thread::spawn(move || run_group(&engine, &targets).unwrap());
    let start = std::time::Instant::now();
    while child.1.progress.lock().unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::yield_now();
    }
    f.engine.cancel(&child.0).unwrap();
    drop(pause);
    worker.join().unwrap();
    assert_eq!(f.names(&parent.0), ["a.txt"]);
    assert_eq!(f.names(&child.0), ["b.txt"]);
    let summary = f.engine.summary().unwrap();
    assert_eq!(
        summary
            .scopes
            .iter()
            .find(|s| s.id == parent.0)
            .unwrap()
            .freshness,
        "current"
    );
    let cancelled = summary.scopes.iter().find(|s| s.id == child.0).unwrap();
    assert_eq!(cancelled.freshness, "partial");
    assert!(cancelled.message.as_ref().unwrap().contains("取消"));
    assert!(child.1.progress.lock().unwrap().is_none());
}

#[test]
fn unchanged_entries_do_not_write_metadata_and_replacements_keep_other_memberships() {
    let f = Fixture::new();
    f.file("child/a.txt");
    let parent = f.scope("", true, &[]);
    let child = f.scope("child", true, &[]);
    run_group(&f.engine, &[parent.clone(), child.clone()]).unwrap();
    let old: i64 = f
        .engine
        .lock()
        .unwrap()
        .query_row("SELECT id FROM entries", [], |r| r.get(0))
        .unwrap();
    f.engine.set_hidden(&[old.to_string()], true).unwrap();
    f.engine.lock().unwrap().execute_batch(
        "CREATE TEMP TABLE writes(n INTEGER); INSERT INTO writes VALUES(0);
         CREATE TEMP TRIGGER count_metadata AFTER UPDATE ON entries BEGIN UPDATE writes SET n=n+1; END;"
    ).unwrap();
    run_group(&f.engine, &[parent.clone(), child.clone()]).unwrap();
    assert_eq!(
        f.engine
            .lock()
            .unwrap()
            .query_row("SELECT n FROM writes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        f.engine
            .query(&model::Query {
                hidden: true,
                ..Default::default()
            })
            .unwrap()
            .entries[0]
            .id,
        old.to_string()
    );
    // Keep the old inode alive to make replacement identity deterministic on all platforms.
    fs::rename(f.root.join("child/a.txt"), f._temp.path().join("old.txt")).unwrap();
    f.file("child/a.txt");
    run(&f.engine, &child.0, &child.1).unwrap();
    let new: i64 = f
        .engine
        .lock()
        .unwrap()
        .query_row("SELECT id FROM entries", [], |r| r.get(0))
        .unwrap();
    assert_ne!(old, new);
    assert_eq!(f.names(&parent.0), ["a.txt"]);
    assert_eq!(f.names(&child.0), ["a.txt"]);
    assert_eq!(
        f.engine
            .query(&model::Query {
                hidden: true,
                ..Default::default()
            })
            .unwrap()
            .total,
        0
    );
}

#[test]
fn changing_one_scope_during_group_scan_does_not_finish_stale_rules() {
    let f = Fixture::new();
    f.file("a.txt");
    f.file("child/b.txt");
    let parent = f.scope("", true, &[]);
    let child = f.scope("child", true, &[]);
    let pause = f.engine.inner.scan_gate.pause();
    let engine = f.engine.clone();
    let targets = [parent.clone(), child.clone()];
    let worker = thread::spawn(move || run_group(&engine, &targets).unwrap());
    let start = std::time::Instant::now();
    while child.1.progress.lock().unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::yield_now();
    }
    f.engine
        .lock()
        .unwrap()
        .execute(
            "UPDATE scopes SET config_version=config_version+1,excludes='[\"*.txt\"]' WHERE id=?",
            [&child.0],
        )
        .unwrap();
    drop(pause);
    worker.join().unwrap();
    assert_eq!(f.names(&parent.0), ["a.txt", "b.txt"]);
    assert!(f.names(&child.0).is_empty());
    let summary = f.engine.summary().unwrap();
    assert_eq!(
        summary
            .scopes
            .iter()
            .find(|s| s.id == parent.0)
            .unwrap()
            .freshness,
        "current"
    );
    assert_eq!(
        summary
            .scopes
            .iter()
            .find(|s| s.id == child.0)
            .unwrap()
            .freshness,
        "partial"
    );
    run(&f.engine, &child.0, &child.1).unwrap();
    assert!(f.names(&child.0).is_empty());
    assert!(f
        .engine
        .summary()
        .unwrap()
        .scopes
        .iter()
        .all(|s| s.freshness == "current"));
}

#[test]
fn cancelled_completion_cannot_delete_old_membership_or_mark_current() {
    let f = Fixture::new();
    f.file("a.txt");
    let scope = f.scope("", true, &[]);
    run(&f.engine, &scope.0, &scope.1).unwrap();
    let version = db::root(&f.engine.lock().unwrap(), &scope.0).unwrap().2;
    f.engine.cancel(&scope.0).unwrap();
    finish_scan(
        &f.engine,
        &scope.0,
        Completion {
            version,
            generation: "cancelled",
            failed: &[],
            count: 0,
            dirty: false,
            watch_failed: false,
            control: Some(&scope.1),
        },
    )
    .unwrap();
    assert_eq!(f.names(&scope.0), ["a.txt"]);
    assert_eq!(f.engine.summary().unwrap().scopes[0].freshness, "partial");
}

#[test]
fn writer_failure_drops_bounded_queue_and_releases_scan_gate() {
    let f = Fixture::new();
    for i in 0..3000 {
        f.file(&format!("file-{i}.txt"));
    }
    // A failure on the first write still sees the full directory total.
    for i in 0..130 {
        fs::create_dir(f.root.join(format!("empty-{i}"))).unwrap();
    }
    let target = f.scope("", true, &[]);
    f.engine.lock().unwrap().execute_batch(
        "CREATE TEMP TRIGGER reject_write BEFORE INSERT ON entries BEGIN SELECT RAISE(ABORT,'test write failure'); END;"
    ).unwrap();
    let (sender, receiver) = mpsc::channel();
    let engine = f.engine.clone();
    let worker = thread::spawn(move || {
        sender.send(run(&engine, &target.0, &target.1)).unwrap();
    });
    let error = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("writer must release blocked producers")
        .unwrap_err();
    assert!(error.to_string().contains("test write failure"));
    worker.join().unwrap();
    let _pause = f.engine.inner.scan_gate.pause();
    assert_eq!(f.engine.summary().unwrap().total, 0);
    let runs = f.engine.inner.scan_runs.lock().unwrap().snapshot();
    assert_eq!(runs[0].total_directories, Some(131));
    assert_eq!(runs[0].checked_files, 0);
    assert_eq!(runs[0].phase, "failed");
}

#[test]
fn separate_pending_roots_share_a_global_plan_and_rescans_have_a_reason() {
    let f = Fixture::new();
    f.file("left/deep/a.txt");
    f.file("right/inner/deeper/b.txt");
    let left = f.scope("left", true, &[]);
    let right = f.scope("right", true, &[]);
    f.engine.scan_pending(&left.0, &left.1).unwrap();
    let summary = f.engine.summary().unwrap();
    assert_eq!(summary.total, 2);
    assert_eq!(summary.scan_runs.len(), 1);
    let progress = &summary.scan_runs[0];
    assert_eq!(progress.scope_ids.len(), 2);
    assert_eq!(progress.total_directories, Some(5));
    assert_eq!(progress.processed_directories, 5);
    assert_eq!(progress.checked_files, 2);
    assert_eq!(progress.phase, "complete");
    assert!(!right.1.pending.lock().unwrap().full);
    left.1
        .pending
        .lock()
        .unwrap()
        .rescan_because("test overflow");
    f.engine.scan_pending(&left.0, &left.1).unwrap();
    let runs = f.engine.inner.scan_runs.lock().unwrap().snapshot();
    assert_eq!(runs[0].round, progress.round + 1);
    assert_eq!(runs[0].reason, "test overflow");
    assert_eq!(runs[0].total_directories, Some(2));
    assert_eq!(runs[0].checked_files, 1);
    assert_eq!(f.engine.summary().unwrap().total, 2);
}

#[test]
fn cancelling_during_directory_count_keeps_old_index_and_no_fake_total() {
    let f = Fixture::new();
    f.file("a/b.txt");
    let target = f.scope("", true, &[]);
    run(&f.engine, &target.0, &target.1).unwrap();
    fs::remove_file(f.root.join("a/b.txt")).unwrap();
    let pause = f.engine.inner.scan_gate.pause();
    let engine = f.engine.clone();
    let next = target.clone();
    let worker = thread::spawn(move || run(&engine, &next.0, &next.1).unwrap());
    let start = std::time::Instant::now();
    while target.1.progress.lock().unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::yield_now();
    }
    let before = f.engine.summary().unwrap();
    assert_eq!(before.scan_runs[0].phase, "counting");
    assert_eq!(before.scan_runs[0].total_directories, None);
    assert_eq!(before.scan_runs[0].checked_files, 0);
    f.engine.cancel(&target.0).unwrap();
    drop(pause);
    worker.join().unwrap();
    assert_eq!(f.names(&target.0), ["b.txt"]);
    assert_eq!(f.engine.summary().unwrap().scan_runs[0].phase, "cancelled");
}

#[test]
fn pause_replays_inflight_wave_without_resurrecting_deleted_files() {
    let f = Fixture::new();
    for i in 0..3000 {
        f.file(&format!("file-{i}.txt"));
    }
    let target = f.scope("", true, &[]);
    // Hold the DB after the reader starts, forcing multiple chunks into its bounded queue.
    let pause = f.engine.inner.scan_gate.pause();
    let engine = f.engine.clone();
    let target2 = target.clone();
    let worker = thread::spawn(move || run(&engine, &target2.0, &target2.1).unwrap());
    let start = std::time::Instant::now();
    while target.1.progress.lock().unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::yield_now();
    }
    // Initialization may still need the DB; wait until it has committed scanning state.
    loop {
        let state: String = f
            .engine
            .lock()
            .unwrap()
            .query_row(
                "SELECT freshness FROM scopes WHERE id=?",
                [&target.0],
                |r| r.get(0),
            )
            .unwrap();
        if state == "scanning" {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::yield_now();
    }
    let db = f.engine.lock().unwrap();
    drop(pause);
    while target.1.progress.lock().unwrap().as_ref().unwrap().1.phase != "indexing" {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::yield_now();
    }
    thread::scope(|threads| {
        let (paused_tx, paused_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let gate = &f.engine.inner.scan_gate;
        threads.spawn(move || {
            let _pause = gate.pause();
            paused_tx.send(()).unwrap();
            resume_rx.recv().unwrap();
        });
        while !f.engine.inner.scan_gate.is_paused() {
            thread::yield_now();
        }
        drop(db);
        paused_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // No observation taken before this boundary may commit afterwards.
        fs::remove_file(f.root.join("file-0.txt")).unwrap();
        f.engine
            .lock()
            .unwrap()
            .execute("DELETE FROM entries WHERE name='file-0.txt'", [])
            .unwrap();
        resume_tx.send(()).unwrap();
    });
    worker.join().unwrap();
    assert_eq!(f.names(&target.0).len(), 2999);
    assert!(!f.names(&target.0).iter().any(|n| n == "file-0.txt"));
    assert_eq!(f.engine.summary().unwrap().scopes[0].scanned, 2999);
}
