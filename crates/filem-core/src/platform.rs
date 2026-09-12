use anyhow::{bail, Result};
use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// Human-readable paths only. File I/O must keep using the native Path/OsString.
pub fn display_path(path: &Path) -> String {
    display_path_text(&path.to_string_lossy())
}

pub fn display_path_text(path: &str) -> String {
    #[cfg(windows)]
    {
        windows_display_path(path)
    }
    #[cfg(not(windows))]
    {
        path.to_owned()
    }
}

#[cfg(any(windows, test))]
fn windows_display_path(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    if let Some(drive) = path.strip_prefix(r"\\?\") {
        let bytes = drive.as_bytes();
        if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":\\" {
            return drive.to_owned();
        }
    }
    // Volume GUID and device namespaces do not have an equivalent drive path.
    path.to_owned()
}

pub fn scoped_directory(scope_name: &str, relative: &Path) -> String {
    let relative = display_path(relative);
    if relative.is_empty() {
        scope_name.to_owned()
    } else {
        format!(
            "{}{}{}",
            scope_name.trim_end_matches(std::path::MAIN_SEPARATOR),
            std::path::MAIN_SEPARATOR,
            relative
        )
    }
}

#[cfg(unix)]
pub fn encode(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}
#[cfg(unix)]
pub fn decode(value: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(value.to_vec())
}
#[cfg(windows)]
pub fn encode(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}
#[cfg(windows)]
pub fn decode(value: &[u8]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    let (units, _) = value.as_chunks::<2>();
    OsString::from_wide(
        &units
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect::<Vec<_>>(),
    )
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub fn mtime(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}
#[cfg(unix)]
pub fn identity(_path: &Path, meta: &fs::Metadata) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("{}:{}", meta.dev(), meta.ino()))
}
#[cfg(unix)]
pub fn file_identity(file: &fs::File) -> Result<String> {
    identity(Path::new(""), &file.metadata()?)
}
#[cfg(windows)]
pub fn file_identity(file: &fs::File) -> Result<String> {
    use std::os::windows::io::AsRawHandle;
    handle_identity(file.as_raw_handle())
}
#[cfg(windows)]
fn handle_identity(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(handle, &mut info) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(format!(
            "{}:{}:{}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        ))
    }
}
#[cfg(windows)]
pub fn identity(path: &Path, _meta: &fs::Metadata) -> Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        Storage::FileSystem::*,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = handle_identity(handle);
        CloseHandle(handle);
        result
    }
}
#[cfg(unix)]
pub fn attributes(_meta: &fs::Metadata) -> u32 {
    0
}
#[cfg(windows)]
pub fn attributes(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}
pub fn placeholder(attrs: u32) -> bool {
    attrs & (0x1000 | 0x40000 | 0x400000) != 0
}
pub fn reparse(attrs: u32) -> bool {
    attrs & 0x400 != 0
}
pub fn checked_root(path: &Path) -> Result<PathBuf> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || reparse(attributes(&meta)) || placeholder(attributes(&meta))
    {
        bail!("请选择链接指向的实际文件夹，首版不跟随符号链接。");
    }
    let root = path.canonicalize()?;
    if !root.is_dir() {
        bail!("监控文件夹必须是文件夹或磁盘根目录。");
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_drive_roots_and_unicode_paths_are_readable() {
        assert_eq!(windows_display_path(r"\\?\D:\"), r"D:\");
        assert_eq!(
            windows_display_path(r"\\?\D:\中文资料\项目 📄.md"),
            r"D:\中文资料\项目 📄.md"
        );
        assert_eq!(windows_display_path(r"D:\中文资料"), r"D:\中文资料");
        let long = format!(r"D:\{}\报告.md", "中文目录\\".repeat(70));
        assert_eq!(windows_display_path(&format!(r"\\?\{long}")), long);
    }

    #[test]
    fn windows_unc_paths_keep_both_leading_separators() {
        assert_eq!(
            windows_display_path(r"\\?\UNC\服务器\共享\资料\报告.pdf"),
            r"\\服务器\共享\资料\报告.pdf"
        );
        assert_eq!(
            windows_display_path(r"\\?\UNC\server\share\"),
            r"\\server\share\"
        );
    }

    #[test]
    fn device_namespaces_and_other_paths_are_not_rewritten() {
        for path in [
            r"\\?\Volume{1234}\资料",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\",
            r"\\.\PhysicalDrive0",
            r"\\?\D:relative",
            r"\\?\中文",
            "/Users/中文/资料",
            "",
        ] {
            assert_eq!(windows_display_path(path), path);
        }
    }

    #[test]
    fn directory_labels_do_not_duplicate_root_separators() {
        let root = format!("D:{}", std::path::MAIN_SEPARATOR);
        assert_eq!(scoped_directory(&root, Path::new("")), root);
        assert_eq!(
            scoped_directory(&root, Path::new("中文")),
            format!("D:{}中文", std::path::MAIN_SEPARATOR)
        );
        assert_eq!(
            scoped_directory("资料", Path::new("项目")),
            format!("资料{}项目", std::path::MAIN_SEPARATOR)
        );
    }

    #[test]
    fn display_conversion_does_not_change_native_path_bytes() {
        let path = Path::new(r"\\?\D:\中文资料\项目 📄.md");
        let before = encode(path.as_os_str());
        let text = display_path(path);
        assert_eq!(decode(&before), path.as_os_str());
        assert_eq!(encode(path.as_os_str()), before);
        #[cfg(windows)]
        assert_eq!(text, r"D:\中文资料\项目 📄.md");
        #[cfg(not(windows))]
        assert_eq!(text, r"\\?\D:\中文资料\项目 📄.md");
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_encoding_preserves_utf16_including_unpaired_surrogates() {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<u16> = r"\\?\D:\中文📄".encode_utf16().collect();
        units.push(0xd800);
        let native = OsString::from_wide(&units);
        assert_eq!(decode(&encode(&native)), native);
    }
}

/// Lossless shell path conversion, tested on every host. Never use a display string for an OS call.
#[cfg(any(windows, test))]
pub(crate) fn shell_path_units(path: &[u16]) -> anyhow::Result<Vec<u16>> {
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
    let slash = b'\\' as u16;
    let result = if path.starts_with(&unc) {
        [vec![slash, slash], path[8..].to_vec()].concat()
    } else if path.starts_with(&prefix) {
        if path.len() < 7
            || !((b'A' as u16..=b'Z' as u16).contains(&path[4])
                || (b'a' as u16..=b'z' as u16).contains(&path[4]))
            || path[5] != b':' as u16
            || path[6] != slash
        {
            anyhow::bail!("此设备路径暂不支持系统文件操作。");
        }
        path[4..].to_vec()
    } else {
        path.to_vec()
    };
    if result.contains(&0) {
        anyhow::bail!("路径包含无效字符。");
    }
    Ok(result)
}

#[cfg(test)]
mod shell_path_tests {
    use super::shell_path_units;
    #[test]
    fn shell_paths_preserve_drive_unc_unicode_and_surrogates() {
        for (input, expected) in [
            (r"\\?\D:\", r"D:\"),
            (r"\\?\D:\中文 资料\a,b%&.txt", r"D:\中文 资料\a,b%&.txt"),
            (r"\\?\UNC\server\share\中文.txt", r"\\server\share\中文.txt"),
        ] {
            assert_eq!(
                shell_path_units(&input.encode_utf16().collect::<Vec<_>>()).unwrap(),
                expected.encode_utf16().collect::<Vec<_>>()
            );
        }
        let mut input: Vec<_> = r"\\?\D:\".encode_utf16().collect();
        input.extend([0xd800, 0xdc00, 0xdfff]);
        assert_eq!(shell_path_units(&input).unwrap(), input[4..]);
        assert!(
            shell_path_units(&r"\\?\Volume{guid}\file".encode_utf16().collect::<Vec<_>>()).is_err()
        );
    }
}
