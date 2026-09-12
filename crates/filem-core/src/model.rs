use serde::{Deserialize, Serialize};

pub const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules",
    ".git",
    "$Recycle.Bin",
    "System Volume Information",
    "Windows/WinSxS",
    "*.tmp",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeInput {
    pub path: String,
    #[serde(default = "yes")]
    pub recursive: bool,
    #[serde(default = "yes")]
    pub watch: bool,
    #[serde(default = "default_excludes")]
    pub excludes: Vec<String>,
}
fn yes() -> bool {
    true
}
fn default_excludes() -> Vec<String> {
    DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub id: String,
    pub name: String,
    pub path: String,
    pub recursive: bool,
    pub watch: bool,
    pub excludes: Vec<String>,
    pub availability: String,
    pub freshness: String,
    pub count: u64,
    pub scanned: u64,
    pub last_scan: Option<i64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Query {
    #[serde(skip)]
    pub candidate_ids: Option<Vec<String>>,
    pub search: String,
    pub scope_id: String,
    pub extension: String,
    pub group: String,
    pub hidden: bool,
    pub min_size: Option<u64>,
    pub modified_after: Option<i64>,
    pub sort: String,
    pub descending: bool,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub path: String,
    pub extension: String,
    pub group: String,
    pub size: u64,
    pub mtime: i64,
    pub scope_id: String,
    pub scope_name: String,
    pub online: bool,
    pub hidden: bool,
    pub placeholder: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub search_engine: String,
    pub search_notice: Option<String>,
    pub entries: Vec<Entry>,
    pub total: u64,
    pub revision: u64,
    pub offset: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub name: String,
    pub count: u64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub facet_scope_id: String,
    pub facet_hidden: bool,
    pub scan_paused: bool,
    pub scopes: Vec<Scope>,
    pub total: u64,
    pub hidden: u64,
    pub size: u64,
    pub extensions: Vec<Bucket>,
    pub hidden_extensions: Vec<Bucket>,
    pub groups: Vec<Bucket>,
    pub revision: u64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteItem {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub status: String,
    pub message: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletePlan {
    pub id: String,
    pub created: i64,
    pub expires: i64,
    pub state: String,
    pub items: Vec<DeleteItem>,
}

pub fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}
pub fn group(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "tiff" | "svg" => "图片",
        "psd" | "ai" | "sketch" | "fig" | "xd" | "afdesign" => "设计源文件",
        "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" => "RAW",
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "flv" | "mp3" | "wav" | "m4a" => "影音",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "md" | "txt" | "csv" => "文档",
        "zip" | "7z" | "rar" | "tar" | "gz" | "zst" => "压缩包",
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "c" | "cpp" | "h" | "java" | "html"
        | "css" | "json" | "toml" | "yaml" | "yml" | "sh" => "代码",
        _ => "其他",
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovePlan {
    #[serde(flatten)]
    pub operation: DeletePlan,
    pub destination: String,
}
