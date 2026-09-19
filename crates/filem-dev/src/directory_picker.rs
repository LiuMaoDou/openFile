//! The browser cannot expose an absolute local directory path. The authenticated
//! loopback dev service opens the OS picker; desktop builds use Tauri's dialog.
use anyhow::{bail, Context, Result};
use std::{path::Path, process::Command, sync::Mutex};

static PICKER: Mutex<()> = Mutex::new(());

pub fn pick() -> Result<Option<String>> {
    let _guard = PICKER
        .try_lock()
        .map_err(|_| anyhow::anyhow!("已有文件夹选择窗口打开，请先完成或取消选择。"))?;
    let output = picker_command()
        .output()
        .context("无法打开系统文件夹选择窗口")?;
    #[cfg(target_os = "linux")]
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        bail!(
            "系统文件夹选择失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_selection(&output.stdout)
}

fn parse_selection(bytes: &[u8]) -> Result<Option<String>> {
    let text = std::str::from_utf8(bytes).context("无法读取系统返回的文件夹路径")?;
    // Remove the command's one line terminator, preserving spaces in names.
    let path = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    if path.is_empty() {
        return Ok(None);
    }
    if !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
        bail!("选择的文件夹不存在或无法访问，请重新选择。");
    }
    Ok(Some(path.to_owned()))
}

#[cfg(target_os = "macos")]
fn picker_command() -> Command {
    let mut command = Command::new("/usr/bin/osascript");
    command.arg("-e").arg(
        r#"
activate
try
    return POSIX path of (choose folder with prompt "选择文件夹")
on error number -128
    return ""
end try
"#,
    );
    command
}

#[cfg(windows)]
fn picker_command() -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new("powershell.exe");
    command.creation_flags(0x08000000);
    command.args([
        "-NoProfile",
        "-STA",
        "-Command",
        r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Add-Type -AssemblyName System.Windows.Forms
$picker = New-Object System.Windows.Forms.FolderBrowserDialog
$picker.Description = 'Select a folder'
$picker.ShowNewFolderButton = $true
try {
    if ($picker.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
        [Console]::WriteLine($picker.SelectedPath)
    }
} finally { $picker.Dispose() }
"#,
    ]);
    command
}

#[cfg(not(any(target_os = "macos", windows)))]
fn picker_command() -> Command {
    let mut command = Command::new("zenity");
    command.args(["--file-selection", "--directory", "--title=选择文件夹"]);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_returns_none_and_invalid_output_is_rejected() {
        assert_eq!(parse_selection(b"\n").unwrap(), None);
        assert_eq!(parse_selection(b"").unwrap(), None);
        assert!(parse_selection(b"relative/path\n").is_err());
        assert!(parse_selection(&[0xff]).is_err());
    }
    #[test]
    fn selected_absolute_folder_is_returned() {
        let folder = std::env::current_dir().unwrap();
        let path = folder.to_str().unwrap();
        assert_eq!(
            parse_selection(format!("{path}\n").as_bytes())
                .unwrap()
                .as_deref(),
            Some(path)
        );
        assert_eq!(
            parse_selection(format!("{path}\r\n").as_bytes())
                .unwrap()
                .as_deref(),
            Some(path)
        );
    }
}
