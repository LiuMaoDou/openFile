//! Text extraction runs in a disposable process, outside the index connection.
use crate::{model::extension, platform::*};
use anyhow::{bail, Context, Result};
use calamine::Reader as WorkbookReader;
use quick_xml::{events::Event, Reader};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Cursor, Read},
    path::PathBuf,
};

pub const MAX_FILE: u64 = 64 * 1024 * 1024;
pub const MAX_TEXT: usize = 2 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
pub(crate) struct Request {
    pub path: Vec<u8>,
    pub identity: String,
    pub size: u64,
    pub mtime: i64,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Extracted {
    pub text: String,
    pub truncated: bool,
    pub note: Option<String>,
}
pub fn supported(ext: &str) -> bool {
    matches!(ext, "pdf" | "docx" | "xlsx" | "pptx") || text_type(ext)
}
fn text_type(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "log"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "py"
            | "go"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "java"
            | "kt"
            | "sh"
            | "bash"
            | "zsh"
            | "sql"
            | "ini"
            | "conf"
            | "cfg"
            | "bat"
            | "ps1"
            | "vue"
            | "svelte"
            | "tex"
            | "rb"
            | "swift"
    )
}
impl Extracted {
    fn append(&mut self, text: &str) {
        let remaining = MAX_TEXT.saturating_sub(self.text.len());
        let mut n = text.len().min(remaining);
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        self.text.push_str(&text[..n]);
        self.truncated |= n < text.len();
    }
}
fn decode_text(bytes: &[u8]) -> Result<(String, Option<String>)> {
    if let Some((encoding, skip)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, errors) = encoding.decode_without_bom_handling(&bytes[skip..]);
        if errors {
            bail!("文本编码损坏，无法可靠解码");
        }
        return Ok((text.into_owned(), None));
    }
    if bytes.contains(&0) {
        bail!("检测到二进制内容或无 BOM 的 UTF-16，请另存为 UTF-8 后重试");
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok((text.into(), None));
    }
    let (text, errors) = encoding_rs::GBK.decode_without_bom_handling(bytes);
    if errors {
        bail!("无法识别文本编码，支持 UTF-8、带 BOM 的 UTF-16 和 GBK");
    }
    Ok((text.into_owned(), Some("按 GBK 解码".into())))
}
fn validate_zip(bytes: &[u8]) -> Result<()> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    if zip.len() > 10_000 {
        bail!("文档内部项目过多，已跳过");
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index(i)?;
        total = total.saturating_add(f.size());
        if total > 128 * 1024 * 1024 || f.size() > 32 * 1024 * 1024 {
            bail!("文档解压大小超过限制，已跳过");
        }
    }
    Ok(())
}
fn xml_text(xml: &str, out: &mut Extracted) -> Result<()> {
    let mut reader = Reader::from_str(xml);
    let mut in_text = false;
    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                in_text = e.local_name().as_ref() == b"t";
            }
            Event::Text(e) if in_text => {
                out.append(&e.decode()?);
            }
            Event::GeneralRef(e) if in_text => {
                let name = e.decode()?;
                out.append(&quick_xml::escape::unescape(&format!("&{name};"))?);
            }
            Event::End(e) => {
                if matches!(e.local_name().as_ref(), b"p" | b"tr") {
                    out.append("\n");
                }
                if e.local_name().as_ref() == b"t" {
                    in_text = false;
                }
            }
            Event::Empty(e) if matches!(e.local_name().as_ref(), b"br" | b"tab") => out.append(" "),
            Event::DocType(_) => bail!("不支持带 DTD 的文档"),
            Event::Eof => break,
            _ => {}
        }
        if out.truncated {
            break;
        }
    }
    Ok(())
}
pub(crate) fn parse(bytes: &[u8], ext: &str) -> Result<Extracted> {
    let mut out = Extracted::default();
    if text_type(ext) {
        let (text, note) = decode_text(bytes)?;
        out.append(&text);
        out.note = note;
    } else if ext == "pdf" {
        let mut doc = pdf_extract::Document::load_mem(bytes).context("PDF 无法读取，可能已损坏")?;
        if doc.is_encrypted() {
            doc.decrypt("").context("PDF 已加密，需要密码")?;
        }
        let unicode_cmap = doc.objects.values().any(|o| {
            o.as_dict()
                .ok()
                .and_then(|d| d.get(b"Encoding").ok())
                .and_then(|v| v.as_name().ok())
                .is_some_and(|name| matches!(name, b"UniGB-UCS2-H" | b"UniGB-UTF16-H"))
        });
        if unicode_cmap {
            // lopdf explicitly supports these standard Unicode CMaps; pdf-extract does not.
            for page in doc.get_pages().keys() {
                out.append(&doc.extract_text(&[*page]).context("PDF 字体编码无法提取")?);
                if out.truncated {
                    break;
                }
            }
        } else {
            struct Limited<'a>(&'a mut Extracted);
            impl std::io::Write for Limited<'_> {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    let text = std::str::from_utf8(bytes).map_err(std::io::Error::other)?;
                    self.0.append(text);
                    if self.0.truncated {
                        return Err(std::io::Error::other("文字提取达到上限"));
                    }
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let result = {
                let mut writer = Limited(&mut out);
                let mut output =
                    pdf_extract::PlainTextOutput::new(&mut writer as &mut dyn std::io::Write);
                pdf_extract::output_doc(&doc, &mut output)
            };
            if !out.truncated {
                result.context("PDF 文字提取失败")?;
            }
        }
    } else {
        validate_zip(bytes)?;
        if ext == "xlsx" {
            let mut book: calamine::Xlsx<_> = calamine::Xlsx::new(Cursor::new(bytes))?;
            for name in book.sheet_names().to_vec() {
                out.append(&format!("\n工作表：{name}\n"));
                let range = book.worksheet_range(&name)?;
                for row in range.rows() {
                    for cell in row {
                        out.append(&cell.to_string());
                        out.append("\t");
                        if out.truncated {
                            break;
                        }
                    }
                    out.append("\n");
                    if out.truncated {
                        break;
                    }
                }
                if out.truncated {
                    break;
                }
            }
        } else if matches!(ext, "docx" | "pptx") {
            let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
            let mut names: Vec<String> = zip
                .file_names()
                .filter(|n| {
                    if ext == "docx" {
                        *n == "word/document.xml"
                            || (n.starts_with("word/header")
                                || n.starts_with("word/footer")
                                || *n == "word/footnotes.xml"
                                || *n == "word/endnotes.xml")
                                && n.ends_with(".xml")
                    } else {
                        (n.starts_with("ppt/slides/slide")
                            || n.starts_with("ppt/notesSlides/notesSlide"))
                            && n.ends_with(".xml")
                    }
                })
                .map(str::to_owned)
                .collect();
            names.sort();
            if names.is_empty() {
                bail!("文档缺少正文 XML，可能已损坏");
            }
            for name in names {
                let mut xml = String::new();
                zip.by_name(&name)?
                    .take(32 * 1024 * 1024 + 1)
                    .read_to_string(&mut xml)?;
                xml_text(&xml, &mut out)?;
                out.append("\n");
                if out.truncated {
                    break;
                }
            }
        } else {
            bail!("此格式暂不支持内容索引");
        }
    }
    // NUL cannot be searched reliably by SQLite text functions.
    out.text = out.text.replace('\0', " ");
    if out.text.trim().is_empty() {
        bail!("未提取到文字；空文档或扫描版 PDF 暂不可搜索，OCR 尚未启用");
    }
    if out.truncated {
        out.note = Some("内容过长，仅索引前 2 MiB 文字".into());
    }
    Ok(out)
}
fn extract_file(request: &Request) -> Result<Extracted> {
    if request.size > MAX_FILE {
        bail!("文件超过 64 MiB，已跳过内容索引");
    }
    let path = PathBuf::from(decode(&request.path));
    if !supported(&extension(
        &path.file_name().unwrap_or_default().to_string_lossy(),
    )) {
        bail!("此格式暂不支持内容索引");
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || reparse(attributes(&meta))
        || placeholder(attributes(&meta))
    {
        bail!("不读取链接或云占位文件");
    }
    let mut file = fs::File::open(&path)?;
    if file_identity(&file)? != request.identity {
        bail!("文件身份已变化");
    }
    let mut bytes = Vec::new();
    file.by_ref().take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    let meta = file.metadata()?;
    if bytes.len() as u64 != request.size
        || meta.len() != request.size
        || mtime(&meta) != request.mtime
    {
        bail!("读取期间文件发生变化");
    }
    parse(
        &bytes,
        &extension(&path.file_name().unwrap_or_default().to_string_lossy()),
    )
}
#[derive(Serialize, Deserialize)]
struct Reply {
    output: Option<Extracted>,
    error: Option<String>,
}
fn read_frame(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut size = [0u8; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > limit {
        bail!("解析消息超过限制");
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn write_frame(writer: &mut impl std::io::Write, bytes: &[u8]) -> Result<()> {
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
}
pub(crate) fn run_helper_if_requested() -> bool {
    let flag = std::env::args().nth(1);
    let stream = flag.as_deref() == Some("--filem-content-helper-stream");
    if !stream && flag.as_deref() != Some("--filem-content-helper") {
        return false;
    }
    let run = || -> Result<()> {
        // Persistent helpers have per-request wall deadlines in the parent;
        // a cumulative CPU limit would incorrectly kill later healthy requests.
        #[cfg(windows)]
        let _resources = limit_resources()?;
        #[cfg(unix)]
        limit_resources(!stream)?;
        let mut input = std::io::stdin().lock();
        let mut output = std::io::stdout().lock();
        loop {
            let request: Request = if stream {
                match read_frame(&mut input, 1024 * 1024) {
                    Ok(bytes) => serde_json::from_slice(&bytes)?,
                    Err(_) => return Ok(()),
                }
            } else {
                serde_json::from_reader((&mut input).take(1024 * 1024))?
            };
            let reply = match extract_file(&request) {
                Ok(output) => Reply {
                    output: Some(output),
                    error: None,
                },
                Err(error) => Reply {
                    output: None,
                    error: Some(format!("{error:#}")),
                },
            };
            if stream {
                write_frame(&mut output, &serde_json::to_vec(&reply)?)?;
            } else {
                serde_json::to_writer(&mut output, &reply)?;
                return Ok(());
            }
        }
    };
    let _ = run();
    true
}
#[cfg(not(test))]
struct Parser {
    child: std::process::Child,
    input: std::process::ChildStdin,
    replies: Option<std::sync::mpsc::Receiver<Result<Vec<u8>>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    used: usize,
}
#[cfg(not(test))]
impl Drop for Parser {
    fn drop(&mut self) {
        self.replies.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
#[cfg(not(test))]
impl Parser {
    fn spawn() -> Result<Self> {
        use std::process::{Command, Stdio};
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--filem-content-helper-stream")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn()?;
        let input = child.stdin.take().context("解析输入不可用")?;
        let mut output = child.stdout.take().context("解析输出不可用")?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || loop {
            let frame = read_frame(&mut output, MAX_TEXT * 6 + 8192);
            let failed = frame.is_err();
            if sender.send(frame).is_err() || failed {
                break;
            }
        });
        Ok(Self {
            child,
            input,
            replies: Some(receiver),
            reader: Some(reader),
            used: 0,
        })
    }
    fn extract(&mut self, request: &Request, valid: impl Fn() -> bool) -> Result<Extracted> {
        use std::{
            sync::mpsc::RecvTimeoutError,
            time::{Duration, Instant},
        };
        if !valid() {
            bail!("内容提取已停止");
        }
        write_frame(&mut self.input, &serde_json::to_vec(request)?)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let bytes = loop {
            // Replies wake immediately; the timeout only checks cancellation and
            // memory, so small documents no longer wait in 50-ms polling steps.
            match self
                .replies
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(50))
            {
                Ok(reply) => break reply?,
                Err(RecvTimeoutError::Disconnected) => bail!("文档解析进程异常退出"),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= deadline || !valid() {
                bail!("内容提取已停止或超过 30 秒限制");
            }
            if parser_memory_exceeded(self.child.id()) {
                bail!("解析内存超过 512 MiB");
            }
        };
        if parser_memory_exceeded(self.child.id()) {
            bail!("解析内存超过 512 MiB");
        }
        let reply: Reply = serde_json::from_slice(&bytes).context("文档解析结果无效")?;
        let result = reply
            .output
            .context(reply.error.unwrap_or_else(|| "文档解析失败".into()))?;
        if result.text.len() > MAX_TEXT {
            bail!("文档解析结果过长");
        }
        self.used += 1;
        Ok(result)
    }
}
#[cfg(not(test))]
pub(crate) fn isolated(request: &Request, valid: impl Fn() -> bool) -> Result<Extracted> {
    thread_local! { static PARSER: std::cell::RefCell<Option<Parser>> = const { std::cell::RefCell::new(None) }; }
    PARSER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|p| p.used >= 64) {
            slot.take();
        }
        if slot.is_none() {
            *slot = Some(Parser::spawn()?);
        }
        let result = slot.as_mut().unwrap().extract(request, valid);
        if result.is_err() {
            slot.take();
        }
        result
    })
}
#[cfg(test)]
pub(crate) fn isolated(request: &Request, valid: impl Fn() -> bool) -> Result<Extracted> {
    if !valid() {
        bail!("内容提取已停止");
    }
    extract_file(request)
}

#[cfg(unix)]
fn limit_resources(limit_cpu: bool) -> Result<()> {
    // macOS may already impose a lower hard data limit. Never raise inherited limits.
    unsafe {
        #[cfg(target_os = "macos")]
        let limits = [(libc::RLIMIT_CPU, 30)];
        #[cfg(not(target_os = "macos"))]
        let limits = [
            (libc::RLIMIT_DATA, 512 * 1024 * 1024),
            (libc::RLIMIT_CPU, 30),
        ];
        for (resource, limit) in limits {
            if resource == libc::RLIMIT_CPU && !limit_cpu {
                continue;
            }
            let mut current: libc::rlimit = std::mem::zeroed();
            if libc::getrlimit(resource, &mut current) != 0 {
                return Err(std::io::Error::last_os_error()).context("无法读取解析资源限制");
            }
            current.rlim_cur = current.rlim_cur.min(current.rlim_max).min(limit);
            if libc::setrlimit(resource, &current) != 0 {
                return Err(std::io::Error::last_os_error()).context("无法限制解析进程资源");
            }
        }
    }
    Ok(())
}
#[cfg(windows)]
struct ParserJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Drop for ParserJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(windows)]
fn limit_resources() -> Result<ParserJob> {
    use windows_sys::Win32::System::{JobObjects::*, Threading::GetCurrentProcess};
    unsafe {
        let job = ParserJob(CreateJobObjectW(std::ptr::null(), std::ptr::null()));
        if job.0.is_null() {
            return Err(std::io::Error::last_os_error()).context("无法创建解析进程资源限制");
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.ProcessMemoryLimit = 512 * 1024 * 1024;
        if SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        ) == 0
            || AssignProcessToJobObject(job.0, GetCurrentProcess()) == 0
        {
            return Err(std::io::Error::last_os_error()).context("无法限制解析进程内存");
        }
        Ok(job)
    }
}

#[cfg(all(not(test), target_os = "macos"))]
fn parser_memory_exceeded(pid: u32) -> bool {
    // macOS rejects RLIMIT_DATA; monitor the actual helper footprint instead.
    unsafe {
        let mut info: libc::rusage_info_v2 = std::mem::zeroed();
        libc::proc_pid_rusage(
            pid as i32,
            libc::RUSAGE_INFO_V2,
            &mut info as *mut _ as *mut _,
        ) == 0
            && info.ri_phys_footprint > 512 * 1024 * 1024
    }
}
#[cfg(all(not(test), not(target_os = "macos")))]
fn parser_memory_exceeded(_: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }
    #[test]
    fn real_office_and_pdf_fixtures_include_english_and_chinese() {
        for ext in ["pdf", "docx", "xlsx", "pptx"] {
            let bytes = fs::read(format!(
                "{}/tests/fixtures/content/sample.{ext}",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
            let out = parse(&bytes, ext).unwrap();
            assert!(out.text.contains("quartzneedle"), "{ext}: {}", out.text);
            assert!(out.text.contains("合同"), "{ext}: {}", out.text);
            if ext == "xlsx" {
                assert!(out.text.contains("12345"));
            }
        }
    }
    #[test]
    fn text_encodings_and_utf8_truncation_are_preserved() {
        for bytes in [
            "合同与 Rust".as_bytes().to_vec(),
            encoding_rs::GBK.encode("合同与 Rust").0.into_owned(),
            [
                vec![255, 254],
                "合同与 Rust"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ]
            .concat(),
        ] {
            assert!(parse(&bytes, "txt").unwrap().text.contains("合同与 Rust"));
        }
        assert!(parse(&[0, 1, 0, 2], "txt").is_err());
        let r = parse("汉".repeat(MAX_TEXT / 3 + 20).as_bytes(), "txt").unwrap();
        assert!(r.truncated);
        assert!(r.text.len() <= MAX_TEXT);
        assert!(r.text.ends_with('汉'));
    }
    #[test]
    fn office_runs_entities_slides_notes_and_invalid_documents() {
        let doc = archive(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="w"><w:p><w:r><w:t>合</w:t></w:r><w:r><w:t>同 &amp; Rust</w:t></w:r></w:p></w:document>"#,
        )]);
        assert!(parse(&doc, "docx").unwrap().text.contains("合同 & Rust"));
        let ppt = archive(&[
            (
                "ppt/slides/slide1.xml",
                r#"<a:p xmlns:a="a"><a:r><a:t>network RDMA</a:t></a:r></a:p>"#,
            ),
            (
                "ppt/notesSlides/notesSlide1.xml",
                r#"<a:p xmlns:a="a"><a:r><a:t>演讲备注</a:t></a:r></a:p>"#,
            ),
        ]);
        let text = parse(&ppt, "pptx").unwrap().text;
        assert!(text.contains("network RDMA"));
        assert!(text.contains("演讲备注"));
        assert!(parse(&archive(&[("not-a-document.txt", "fake")]), "docx").is_err());
        assert!(parse(b"broken zip", "docx").is_err());
        assert!(parse(b"broken PDF", "pdf").is_err());
    }
}
