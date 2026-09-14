use filem_core::{
    model::{extension, Query, ScopeInput},
    Engine,
};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn input(root: &Path) -> ScopeInput {
    ScopeInput {
        content_enabled: false,
        path: root.to_string_lossy().into(),
        recursive: true,
        watch: false,
        excludes: vec!["node_modules".into(), "*.tmp".into()],
    }
}
fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}
fn setup() -> (TempDir, Engine, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    fs::create_dir_all(&root).unwrap();
    let engine = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
    (temp, engine, root)
}
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
            start.elapsed() < Duration::from_secs(15),
            "scope failed to complete: {scope:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn names(engine: &Engine, q: Query) -> Vec<String> {
    engine
        .query(&q)
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.name)
        .collect()
}

#[test]
fn suffixes_are_normalized_without_misclassifying_dotfiles() {
    assert_eq!(extension("PHOTO.PNG"), "png");
    assert_eq!(extension(".gitignore"), "");
    assert_eq!(extension("README"), "");
    assert_eq!(extension("a.tar.gz"), "gz");
}

#[test]
fn unicode_paths_survive_queries_delete_previews_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("中文资料 📁");
    let source = root.join("子目录").join("项目方案 📄.md");
    write(&source, "# 中文内容\n");
    let database = temp.path().join("state/index.sqlite");
    let engine = Engine::open(&database).unwrap();
    let id = engine.add_scope(input(&root)).unwrap();
    wait(&engine, &id);
    let stored = rusqlite::Connection::open(&database).unwrap();
    let native_before: Vec<u8> = stored
        .query_row("SELECT root FROM scopes WHERE id=?", [&id], |r| r.get(0))
        .unwrap();
    let scope = engine.summary().unwrap().scopes.remove(0);
    assert_eq!(scope.name, "中文资料 📁");
    assert!(!scope.path.starts_with(r"\\?\"));
    let entry = engine.query(&Query::default()).unwrap().entries.remove(0);
    assert_eq!(entry.name, "项目方案 📄.md");
    assert!(entry.path.ends_with(&format!(
        "中文资料 📁{}子目录{}项目方案 📄.md",
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    )));
    assert!(!entry.path.starts_with(r"\\?\"));
    assert_eq!(engine.preview_text(&entry.id).unwrap().text, "# 中文内容\n");
    let plan = engine
        .preview_delete(std::slice::from_ref(&entry.id))
        .unwrap();
    assert_eq!(plan.items[0].path, entry.path);
    let json = serde_json::to_string(&plan).unwrap();
    assert!(json.contains("项目方案 📄.md"));
    assert!(!json.contains('\u{fffd}'));
    drop(engine);
    let engine = Engine::open(&database).unwrap();
    wait(&engine, &id);
    assert_eq!(engine.summary().unwrap().scopes[0].path, scope.path);
    assert_eq!(
        engine.delete_plan(&plan.id).unwrap().items[0].path,
        entry.path
    );
    assert_eq!(
        engine.query(&Query::default()).unwrap().entries[0].id,
        entry.id
    );
    let native_after: Vec<u8> = stored
        .query_row("SELECT root FROM scopes WHERE id=?", [&id], |r| r.get(0))
        .unwrap();
    assert_eq!(native_before, native_after);
    assert_eq!(fs::read_to_string(source).unwrap(), "# 中文内容\n");
}

#[test]
fn displayed_path_can_be_reused_to_edit_an_offline_scope() {
    let (temp, engine, root) = setup();
    write(&root.join("资料.md"), "keep");
    let id = engine.add_scope(input(&root)).unwrap();
    wait(&engine, &id);
    let scope = engine.summary().unwrap().scopes.remove(0);
    let detached = temp.path().join("offline");
    fs::rename(&root, &detached).unwrap();
    let mut config = input(&root);
    config.path = scope.path;
    config.recursive = false;
    engine.update_scope(&id, config).unwrap();
    assert!(!engine.summary().unwrap().scopes[0].recursive);
    assert_eq!(
        fs::read_to_string(detached.join("资料.md")).unwrap(),
        "keep"
    );
}
#[test]
fn real_scan_excludes_and_recursive_changes_are_reconciled() {
    let (_tmp, e, root) = setup();
    write(&root.join("hello.MD"), "hello");
    write(&root.join("nested/中文.md"), "中文");
    write(&root.join("nested/ignore.tmp"), "ignored");
    write(&root.join("node_modules/package.json"), "{}");
    let id = e.add_scope(input(&root)).unwrap();
    wait(&e, &id);
    assert_eq!(e.summary().unwrap().total, 2);
    let q = Query {
        extension: "md".into(),
        ..Default::default()
    };
    assert_eq!(e.query(&q).unwrap().total, 2);
    let mut config = input(&root);
    config.recursive = false;
    e.update_scope(&id, config).unwrap();
    wait(&e, &id);
    assert_eq!(names(&e, Query::default()), vec!["hello.MD"]);
    assert!(root.join("nested/中文.md").exists());
}
#[test]
fn overlapping_scopes_preserve_membership_and_use_innermost_allowed_owner() {
    let (_tmp, e, root) = setup();
    write(&root.join("Final/logo.md"), "logo");
    write(&root.join("Final/spec.txt"), "spec");
    let parent = e.add_scope(input(&root)).unwrap();
    wait(&e, &parent);
    let mut config = input(&root.join("Final"));
    config.excludes.push("*.md".into());
    let child = e.add_scope(config.clone()).unwrap();
    wait(&e, &child);
    assert_eq!(e.summary().unwrap().total, 2);
    let entries = e.query(&Query::default()).unwrap().entries;
    assert_eq!(
        entries
            .iter()
            .find(|e| e.name == "logo.md")
            .unwrap()
            .scope_id,
        parent
    );
    assert_eq!(
        entries
            .iter()
            .find(|e| e.name == "spec.txt")
            .unwrap()
            .scope_id,
        child
    );
    assert_eq!(
        e.query(&Query {
            scope_id: parent.clone(),
            ..Default::default()
        })
        .unwrap()
        .total,
        2
    );
    assert_eq!(
        e.query(&Query {
            scope_id: child.clone(),
            ..Default::default()
        })
        .unwrap()
        .total,
        1
    );
    config.excludes.clear();
    e.update_scope(&child, config).unwrap();
    wait(&e, &child);
    assert!(e
        .query(&Query::default())
        .unwrap()
        .entries
        .iter()
        .all(|e| e.scope_id == child));
    e.remove_scope(&child).unwrap();
    assert_eq!(e.summary().unwrap().total, 2);
    e.remove_scope(&parent).unwrap();
    assert_eq!(e.summary().unwrap().total, 0);
    assert_eq!(
        fs::read_to_string(root.join("Final/logo.md")).unwrap(),
        "logo"
    );
}
#[test]
fn hide_restore_is_persistent_and_never_changes_the_source() {
    let (tmp, e, root) = setup();
    write(&root.join("readme.md"), "# Keep me\n");
    let id = e.add_scope(input(&root)).unwrap();
    wait(&e, &id);
    let entry = e.query(&Query::default()).unwrap().entries.remove(0);
    e.set_hidden(std::slice::from_ref(&entry.id), true).unwrap();
    assert_eq!(e.summary().unwrap().total, 0);
    assert_eq!(e.summary().unwrap().hidden, 1);
    assert!(e.summary().unwrap().extensions.is_empty());
    assert_eq!(e.summary().unwrap().hidden_extensions[0].name, "md");
    assert_eq!(e.summary().unwrap().hidden_extensions[0].count, 1);
    drop(e);
    let e = Engine::open(tmp.path().join("state/index.sqlite")).unwrap();
    wait(&e, &id);
    assert_eq!(e.summary().unwrap().hidden, 1);
    e.set_hidden(&[entry.id], false).unwrap();
    assert_eq!(e.summary().unwrap().total, 1);
    assert_eq!(
        fs::read_to_string(root.join("readme.md")).unwrap(),
        "# Keep me\n"
    );
}
#[test]
fn restart_reconciles_deep_modifications_and_deletions() {
    let (tmp, e, root) = setup();
    write(&root.join("A/B/change.md"), "old");
    write(&root.join("A/B/gone.txt"), "gone");
    let id = e.add_scope(input(&root)).unwrap();
    wait(&e, &id);
    drop(e);
    write(&root.join("A/B/change.md"), "a much longer new body");
    fs::remove_file(root.join("A/B/gone.txt")).unwrap();
    write(&root.join("A/B/new.txt"), "new");
    let e = Engine::open(tmp.path().join("state/index.sqlite")).unwrap();
    wait(&e, &id);
    assert_eq!(e.summary().unwrap().total, 2);
    let file = e
        .query(&Query {
            search: "change".into(),
            ..Default::default()
        })
        .unwrap()
        .entries
        .remove(0);
    assert_eq!(file.size, 22);
}
#[test]
fn replacement_invalidates_old_ids_but_preserves_other_scope_memberships() {
    let (tmp, e, root) = setup();
    write(&root.join("child/a.md"), "first");
    let parent = e.add_scope(input(&root)).unwrap();
    wait(&e, &parent);
    let child = e.add_scope(input(&root.join("child"))).unwrap();
    wait(&e, &child);
    let old = e.query(&Query::default()).unwrap().entries.remove(0).id;
    fs::rename(root.join("child/a.md"), tmp.path().join("held.md")).unwrap();
    write(&root.join("child/a.md"), "next");
    assert!(e.preview_text(&old).is_err());
    e.refresh(&child).unwrap();
    wait(&e, &child);
    let new = e.query(&Query::default()).unwrap().entries.remove(0);
    assert_ne!(old, new.id);
    assert_eq!(
        e.query(&Query {
            scope_id: parent,
            ..Default::default()
        })
        .unwrap()
        .total,
        1
    );
    assert!(e.set_hidden(&[old], true).is_err());
    assert_eq!(e.preview_text(&new.id).unwrap().text, "next");
}
#[test]
fn offline_scope_keeps_index_and_blocks_content_reads() {
    let (tmp, e, root) = setup();
    write(&root.join("a.md"), "body");
    let id = e.add_scope(input(&root)).unwrap();
    wait(&e, &id);
    let file = e.query(&Query::default()).unwrap().entries.remove(0);
    fs::rename(&root, tmp.path().join("disconnected")).unwrap();
    e.refresh(&id).unwrap();
    let start = Instant::now();
    while e.summary().unwrap().scopes[0].availability != "offline" {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(e.summary().unwrap().total, 1);
    assert!(!e.query(&Query::default()).unwrap().entries[0].online);
    assert!(e.preview_text(&file.id).is_err());
}
#[test]
fn queries_are_paged_and_user_input_is_not_sql() {
    let (_tmp, e, root) = setup();
    for i in 0..30 {
        write(&root.join(format!("file-{i:02}.txt")), &"x".repeat(i + 1));
    }
    let id = e.add_scope(input(&root)).unwrap();
    wait(&e, &id);
    let page = e
        .query(&Query {
            limit: 7,
            offset: 7,
            sort: "size".into(),
            descending: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 30);
    assert_eq!(page.entries.len(), 7);
    assert_eq!(page.entries[0].size, 23);
    assert_eq!(
        e.query(&Query {
            search: "%' OR 1=1 --".into(),
            ..Default::default()
        })
        .unwrap()
        .total,
        0
    );
    assert_eq!(
        e.query(&Query {
            min_size: Some(28),
            ..Default::default()
        })
        .unwrap()
        .total,
        3
    );
}
#[test]
fn directory_move_updates_descendants_without_losing_files() {
    let (_tmp, e, root) = setup();
    write(&root.join("before/deep/a.md"), "body");
    let id = e.add_scope(input(&root)).unwrap();
    wait(&e, &id);
    fs::rename(root.join("before"), root.join("after")).unwrap();
    e.refresh(&id).unwrap();
    wait(&e, &id);
    let page = e.query(&Query::default()).unwrap();
    assert_eq!(page.total, 1);
    assert!(Path::new(&page.entries[0].directory).ends_with(Path::new("after").join("deep")));
}
#[test]
fn native_watch_reconciles_created_files() {
    let (_tmp, e, root) = setup();
    write(&root.join("a.md"), "first");
    let mut config = input(&root);
    config.watch = true;
    let id = e.add_scope(config).unwrap();
    wait(&e, &id);
    let last_scan = e.summary().unwrap().scopes[0].last_scan;
    write(&root.join("new.md"), "second");
    let start = Instant::now();
    loop {
        if e.summary().unwrap().total == 2 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(12),
            "native watcher did not reconcile"
        );
        thread::sleep(Duration::from_millis(50));
    }
    wait(&e, &id);
    assert_eq!(
        e.summary().unwrap().scopes[0].last_scan,
        last_scan,
        "a file event must not restart the full scan"
    );
}
#[cfg(unix)]
#[test]
fn symlink_escape_and_loop_are_not_followed() {
    use std::os::unix::fs::symlink;
    let (tmp, e, root) = setup();
    let outside = tmp.path().join("outside");
    write(&outside.join("secret.md"), "not authorized");
    write(&root.join("ok.md"), "allowed");
    symlink(&outside, root.join("escape")).unwrap();
    symlink(&root, root.join("loop")).unwrap();
    symlink(outside.join("secret.md"), root.join("link.md")).unwrap();
    let id = e.add_scope(input(&root)).unwrap();
    wait(&e, &id);
    assert_eq!(names(&e, Query::default()), vec!["ok.md"]);
    assert!(e.add_scope(input(&root.join("escape"))).is_err());
}
#[test]
fn corrupt_database_is_not_silently_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("index.sqlite");
    fs::write(&path, "corrupt data").unwrap();
    assert!(Engine::open(&path).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "corrupt data");
}

#[test]
fn folder_facets_are_scoped_without_duplicate_overlap_counts() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("a.txt"), "a").unwrap();
    std::fs::write(b.join("b.png"), "b").unwrap();
    let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
    let input = |path: &std::path::Path| ScopeInput {
        content_enabled: false,
        path: path.to_string_lossy().into(),
        recursive: true,
        watch: false,
        excludes: vec![],
    };
    let first = e.add_scope(input(&a)).unwrap();
    let second = e.add_scope(input(&b)).unwrap();
    let start = std::time::Instant::now();
    while e
        .summary()
        .unwrap()
        .scopes
        .iter()
        .any(|s| s.freshness != "current")
    {
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let facets = e
        .dispatch("summary", serde_json::json!({"scopeId":first}))
        .unwrap();
    assert_eq!(
        facets["groups"],
        serde_json::json!([{"name":"文档","count":1}])
    );
    assert_eq!(
        facets["extensions"],
        serde_json::json!([{"name":"txt","count":1}])
    );
    let facets = e
        .dispatch("summary", serde_json::json!({"scopeId":second}))
        .unwrap();
    assert_eq!(
        facets["groups"],
        serde_json::json!([{"name":"图片","count":1}])
    );
    assert_eq!(
        facets["extensions"],
        serde_json::json!([{"name":"png","count":1}])
    );
    assert_eq!(facets["total"], 2);
}
