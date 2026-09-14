//! Optional Everything IPC acceleration. SDK queries run in a bounded helper process.
#[cfg(windows)]
use crate::platform::*;
use crate::{db, model::*, Engine};
#[cfg(windows)]
use anyhow::Context;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Serialize, Deserialize, Default)]
struct Request {
    roots: Vec<Vec<u16>>,
    #[serde(default)]
    short_roots: Vec<Vec<u16>>,
    search: String,
    limit: u32,
    probe: bool,
}
#[derive(Serialize, Deserialize, Default)]
struct Reply {
    available: bool,
    paths: Vec<Vec<u16>>,
    truncated: bool,
    error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported: bool,
    pub available: bool,
    pub enabled: bool,
    pub message: String,
}

impl Engine {
    pub fn everything_status(&self) -> Status {
        let enabled = self.everything_enabled();
        let available = cfg!(windows)
            && request(Request {
                probe: true,
                ..Default::default()
            })
            .is_ok_and(|r| r.available);
        Status {
            supported: cfg!(windows),
            available,
            enabled,
            message: if !cfg!(windows) {
                "Everything SDK 仅支持 Windows；当前使用本地索引。"
            } else if available {
                "Everything 已连接，可加速初次发现文件与文件名搜索。"
            } else {
                "请安装并运行 Everything 1.4 标准版（非 Lite），并等待它完成索引；当前自动使用本地索引。"
            }
            .into(),
        }
    }
    pub(crate) fn everything_enabled(&self) -> bool {
        self.inner
            .query_db
            .lock()
            .ok()
            .and_then(|c| {
                c.query_row(
                    "SELECT value FROM settings WHERE key='everything'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
            })
            .as_deref()
            == Some("1")
    }
    pub fn set_everything(&self, enabled: bool) -> Result<Status> {
        if enabled && !cfg!(windows) {
            bail!("Everything SDK 仅支持 Windows。");
        }
        self.lock()?.execute("INSERT INTO settings(key,value) VALUES('everything',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if enabled{"1"}else{"0"}])?;
        if enabled {
            for scope in self.summary()?.scopes {
                self.refresh(&scope.id)?;
            }
        }
        Ok(self.everything_status())
    }
    pub(crate) fn everything_prefill(&self, scope_id: &str) -> Result<()> {
        if !self.everything_enabled() {
            return Ok(());
        }
        let root = {
            let c = self.lock()?;
            let count: u64 = c.query_row(
                "SELECT COUNT(*) FROM memberships WHERE scope_id=?",
                [scope_id],
                |r| r.get(0),
            )?;
            if count > 0 {
                return Ok(());
            }
            db::root(&c, scope_id)?.0
        };
        let result = paths(&[root], "", 2000)?;
        crate::scan::index_candidates(self, scope_id, &result.0)?;
        Ok(())
    }
    pub(crate) fn query_with_everything(&self, q: &Query) -> Result<QueryResult> {
        if !self.everything_enabled() || q.search.is_empty() {
            return self.query_local(q);
        }
        // Cover filesystem validation as well as IPC. Rapid typing must not
        // launch several concurrent 50,000-path stat passes.
        let Ok(_query) = self.inner.everything_query.try_lock() else {
            let mut result = self.query_local(q)?;
            result.search_notice = Some("Everything 正在处理另一个搜索，已使用本地索引。".into());
            return Ok(result);
        };
        let scopes = {
            let c = self.lock()?;
            let mut s = c.prepare("SELECT id FROM scopes WHERE ?='' OR id=?")?;
            let ids = s
                .query_map([&q.scope_id, &q.scope_id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter()
                .map(|id| Ok((id.clone(), db::root(&c, &id)?.0)))
                .collect::<Result<Vec<_>>>()?
        };
        if scopes.is_empty() {
            return self.query_local(q);
        }
        let lookup = (|| -> Result<Vec<String>> {
            let (paths, truncated) = paths(
                &scopes
                    .iter()
                    .map(|(_, root)| root.clone())
                    .collect::<Vec<_>>(),
                &q.search,
                50000,
            )?;
            if truncated {
                bail!("匹配文件超过 50,000 个，使用完整本地索引查询。");
            }
            let mut ids = std::collections::HashSet::new();
            for (scope, _) in scopes {
                ids.extend(crate::scan::index_candidates(self, &scope, &paths)?);
            }
            Ok(ids.into_iter().collect())
        })();
        match lookup {
            Ok(ids) => {
                let mut query = q.clone();
                query.search.clear();
                query.candidate_ids = Some(ids);
                let mut result = self.query_local(&query)?;
                if result.total == 0 {
                    let mut local = self.query_local(q)?;
                    if local.total > 0 {
                        local.search_notice =
                            Some("Everything 尚未覆盖这些结果，已使用本地索引。".into());
                        return Ok(local);
                    }
                }
                result.search_engine = "everything".into();
                Ok(result)
            }
            Err(error) => {
                let mut result = self.query_local(q)?;
                result.search_notice =
                    Some(format!("Everything 不可用，已使用本地索引：{error:#}"));
                Ok(result)
            }
        }
    }
}
#[cfg(windows)]
fn paths(roots: &[PathBuf], search: &str, limit: u32) -> Result<(Vec<PathBuf>, bool)> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let request = Request {
        short_roots: roots
            .iter()
            .map(|p| short_root(p).unwrap_or_default())
            .collect(),
        roots: roots
            .iter()
            .map(|p| shell_path_units(&p.as_os_str().encode_wide().collect::<Vec<_>>()))
            .collect::<Result<_>>()?,
        search: search.into(),
        limit,
        probe: false,
    };
    let result = request_fn(request)?;
    Ok((
        result
            .paths
            .into_iter()
            .map(|p| PathBuf::from(std::ffi::OsString::from_wide(&p)))
            .collect(),
        result.truncated,
    ))
}
#[cfg(windows)]
fn short_root(path: &std::path::Path) -> Option<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let size = unsafe { GetShortPathNameW(input.as_ptr(), std::ptr::null_mut(), 0) };
    if size == 0 || size > 32768 {
        return None;
    }
    let mut output = vec![0; size as usize];
    let len = unsafe { GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), size) };
    if len == 0 || len >= size {
        return None;
    }
    output.truncate(len as usize);
    shell_path_units(&output).ok()
}
#[cfg(not(windows))]
fn paths(_roots: &[PathBuf], _search: &str, _limit: u32) -> Result<(Vec<PathBuf>, bool)> {
    bail!("当前系统不支持 Everything SDK。")
}
fn request(input: Request) -> Result<Reply> {
    request_fn(input)
}
#[cfg(not(windows))]
fn request_fn(_input: Request) -> Result<Reply> {
    bail!("Everything SDK 仅支持 Windows。")
}

#[cfg(windows)]
fn request_fn(input: Request) -> Result<Reply> {
    static QUERY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _query = QUERY
        .try_lock()
        .map_err(|_| anyhow::anyhow!("SDK 正在处理另一个查询，暂用本地索引。"))?;
    use std::{
        io::{Read, Write},
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct Helper(std::process::Child);
    impl Drop for Helper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut helper = Helper(
        Command::new(std::env::current_exe()?)
            .arg("--filem-everything-helper")
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let child = &mut helper.0;
    let mut stdin = child.stdin.take().context("Everything 查询输入不可用")?;
    serde_json::to_writer(&mut stdin, &input)?;
    stdin.flush()?;
    drop(stdin);
    let stdout = child.stdout.take().context("Everything 查询输出不可用")?;
    let reader = std::thread::spawn(move || {
        let mut data = Vec::new();
        stdout
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .map(|_| data)
    });
    let start = Instant::now();
    let result = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(6) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("查询超时，已结束此次 SDK 查询。");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Everything 查询线程退出"))??;
    if !result.success() || bytes.len() > 64 * 1024 * 1024 {
        bail!("Everything 查询未成功完成。");
    }
    let reply: Reply = serde_json::from_slice(&bytes).context("Everything 查询响应无效")?;
    if let Some(error) = &reply.error {
        bail!("{error}");
    }
    Ok(reply)
}

// Keep arbitrary search text literal, and force matching below the selected roots.
#[cfg(any(windows, test))]
fn query_pattern(roots: &[Vec<u16>], short_roots: &[Vec<u16>], search: &str) -> Vec<u16> {
    fn literal(units: &[u16]) -> Vec<u16> {
        let meta: Vec<u16> = r"\.^$|?*+()[]{}".encode_utf16().collect();
        let mut out = Vec::new();
        for unit in units {
            if meta.contains(unit) {
                out.push(b'\\' as u16);
            }
            out.push(*unit);
        }
        out
    }
    let mut out: Vec<u16> = "^(?=.*".encode_utf16().collect();
    out.extend(literal(&search.encode_utf16().collect::<Vec<_>>()));
    out.extend(")(?:".encode_utf16());
    for (i, root) in roots.iter().enumerate() {
        if i > 0 {
            out.push(b'|' as u16);
        }
        let mut root = root.clone();
        if root.last() != Some(&(b'\\' as u16)) {
            root.push(b'\\' as u16);
        }
        let mut short = short_roots.get(i).cloned().unwrap_or_default();
        if short.last() != Some(&(b'\\' as u16)) {
            short.push(b'\\' as u16);
        }
        let components = root.split(|c| *c == b'\\' as u16).collect::<Vec<_>>();
        let aliases = short.split(|c| *c == b'\\' as u16).collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            if index > 0 {
                out.extend(literal(&[b'\\' as u16]));
            }
            // Everything folder indexes can retain a mixture of long names and
            // 8.3 aliases. Match either spelling at each level, without expanding
            // the number of complete root alternatives exponentially.
            if aliases.len() == components.len()
                && aliases[index] != *component
                && !aliases[index].is_empty()
            {
                out.extend("(?:".encode_utf16());
                out.extend(literal(component));
                out.push(b'|' as u16);
                out.extend(literal(aliases[index]));
                out.push(b')' as u16);
            } else {
                out.extend(literal(component));
            }
        }
    }
    out.extend(")".encode_utf16());
    out.push(0);
    out
}
/// Called before starting Tauri or the development server. It never opens or changes files.
pub fn run_helper_if_requested() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--filem-everything-helper") {
        return false;
    }
    #[cfg(windows)]
    {
        let reply = helper().unwrap_or_else(|e| Reply {
            error: Some(format!("{e:#}")),
            ..Default::default()
        });
        let _ = serde_json::to_writer(std::io::stdout(), &reply);
    }
    true
}
#[cfg(windows)]
fn helper() -> Result<Reply> {
    use std::io::Read;
    #[link(name = "filem_everything", kind = "static")]
    unsafe extern "system" {
        fn Everything_IsDBLoaded() -> i32;
        fn Everything_Reset();
        fn Everything_SetSearchW(text: *const u16);
        fn Everything_SetMatchPath(value: i32);
        fn Everything_SetRegex(value: i32);
        fn Everything_SetMax(value: u32);
        fn Everything_SetRequestFlags(value: u32);
        fn Everything_QueryW(wait: i32) -> i32;
        fn Everything_GetLastError() -> u32;
        fn Everything_GetNumResults() -> u32;
        fn Everything_GetTotResults() -> u32;
        fn Everything_IsFileResult(index: u32) -> i32;
        fn Everything_GetResultFullPathNameW(index: u32, buffer: *mut u16, size: u32) -> u32;
    }
    let input: Request = serde_json::from_reader(std::io::stdin().take(1024 * 1024))?;
    unsafe {
        if Everything_IsDBLoaded() == 0 {
            return Ok(Reply {
                error: Some("Everything 未运行或索引尚未加载。".into()),
                ..Default::default()
            });
        }
        if input.probe {
            return Ok(Reply {
                available: true,
                ..Default::default()
            });
        }
        if input.roots.is_empty()
            || input.roots.len() > 32
            || input.search.len() > 4096
            || input.limit == 0
            || input.limit > 50000
        {
            bail!("查询参数无效。");
        }
        let pattern = query_pattern(&input.roots, &input.short_roots, &input.search);
        Everything_Reset();
        Everything_SetMatchPath(1);
        Everything_SetRegex(1);
        Everything_SetMax(input.limit);
        Everything_SetRequestFlags(0x00000001 | 0x00000002);
        Everything_SetSearchW(pattern.as_ptr());
        if Everything_QueryW(1) == 0 {
            bail!("SDK 查询失败，错误码 {}", Everything_GetLastError());
        }
        let mut paths = Vec::new();
        for i in 0..Everything_GetNumResults() {
            if Everything_IsFileResult(i) == 0 {
                continue;
            }
            let len = Everything_GetResultFullPathNameW(i, std::ptr::null_mut(), 0);
            if len == 0 || len > 32767 {
                continue;
            }
            let mut name = vec![0; len as usize + 1];
            let copied = Everything_GetResultFullPathNameW(i, name.as_mut_ptr(), name.len() as u32);
            if copied == 0 || copied > len {
                bail!("SDK 返回了不完整路径。");
            }
            name.truncate(copied as usize);
            paths.push(name);
        }
        Ok(Reply {
            available: true,
            paths,
            truncated: Everything_GetTotResults() > input.limit,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_regex_quotes_search_metacharacters_and_preserves_unc() {
        let roots = vec![
            r"D:\素材".encode_utf16().collect(),
            r"\\server\共享\".encode_utf16().collect(),
        ];
        let pattern = query_pattern(&roots, &[], "[draft].txt|C:");
        let text = String::from_utf16(&pattern[..pattern.len() - 1]).unwrap();
        assert_eq!(
            text,
            r"^(?=.*\[draft\]\.txt\|C:)(?:D:\\素材\\|\\\\server\\共享\\)"
        );
        let mut root = r"D:\".encode_utf16().collect::<Vec<_>>();
        root.push(0xdfff);
        assert!(query_pattern(&[root], &[], "").contains(&0xdfff));
    }
    #[test]
    fn root_regex_accepts_mixed_short_and_long_component_spellings() {
        let long = r"C:\Users\runneradmin\中文 资料".encode_utf16().collect();
        let short = r"C:\Users\RUNNER~1\ABCDEF~1".encode_utf16().collect();
        let pattern = query_pattern(&[long], &[short], "draft");
        assert_eq!(
            String::from_utf16(&pattern[..pattern.len() - 1]).unwrap(),
            r"^(?=.*draft)(?:C:\\Users\\(?:runneradmin|RUNNER~1)\\(?:中文 资料|ABCDEF~1)\\)"
        );
    }
    #[cfg(not(windows))]
    #[test]
    fn unavailable_sdk_preserves_local_search() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), "alpha content").unwrap();
        let e = Engine::open(temp.path().join("state/index.sqlite")).unwrap();
        e.add_scope(ScopeInput {
            content_enabled: false,
            path: root.to_str().unwrap().into(),
            recursive: true,
            watch: false,
            excludes: vec![],
        })
        .unwrap();
        let start = std::time::Instant::now();
        while e.summary().unwrap().scopes[0].freshness != "current" {
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let status = e.everything_status();
        assert!(!status.supported && !status.available);
        assert!(e.set_everything(true).is_err());
        // Simulate an enabled preference restored from Windows, so the test
        // exercises the unavailable backend branch rather than disabled mode.
        e.lock()
            .unwrap()
            .execute(
                "INSERT INTO settings(key,value) VALUES('everything','1')",
                [],
            )
            .unwrap();
        let result = e
            .query(&Query {
                search: "alpha".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.search_engine, "local");
        assert_eq!(result.entries.len(), 1);
        assert!(result.search_notice.unwrap().contains("Everything 不可用"));
        let _busy = e.inner.everything_query.lock().unwrap();
        let result = e
            .query(&Query {
                search: "a".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(result.search_notice.unwrap().contains("另一个搜索"));
    }
}
