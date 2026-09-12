//! Shell APIs accept ordinary absolute paths/PIDLs, not Rust's verbatim filesystem paths.
use crate::platform::{identity, shell_path_units};
use anyhow::{bail, Context, Result};
use std::{
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use windows::{
    core::{implement, Ref, HRESULT, PCWSTR},
    Win32::{
        Foundation::E_ABORT,
        System::Com::*,
        UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL},
    },
};

fn in_apartment(action: impl FnOnce() -> Result<()> + Send + 'static) -> Result<()> {
    std::thread::spawn(move || {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        action()
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Windows 文件操作线程退出，请查看操作记录。"))?
}

fn shell_item(path: &Path) -> Result<IShellItem> {
    // Work in UTF-16, including names that cannot be represented losslessly as UTF-8.
    let native: Vec<u16> = path.as_os_str().encode_wide().collect();
    let mut wide = shell_path_units(&native)?;
    wide.push(0);
    let expected = identity(path, &fs::symlink_metadata(path)?)?;
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
            .context("Windows 无法解析目标路径")?;
        let parsed = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let parsed_path = PathBuf::from(std::ffi::OsString::from_wide(parsed.as_wide()));
        CoTaskMemFree(Some(parsed.0.cast()));
        let resolved = fs::symlink_metadata(&parsed_path).context("Windows 解析的文件不可访问")?;
        if identity(&parsed_path, &resolved)? != expected
            || parsed_path.canonicalize()? != path.canonicalize()?
        {
            bail!("Windows 解析后的目标与原文件不一致，已停止操作。请在资源管理器中检查该文件名。");
        }
        Ok(item)
    }
}

pub(crate) fn open(path: &Path, reveal: bool) -> Result<()> {
    let path = path.to_path_buf();
    in_apartment(move || unsafe {
        let item = shell_item(&path)?;
        let pidl = SHGetIDListFromObject(&item)?;
        struct Pidl(*mut Common::ITEMIDLIST);
        impl Drop for Pidl {
            fn drop(&mut self) {
                unsafe {
                    CoTaskMemFree(Some(self.0.cast()));
                }
            }
        }
        let pidl = Pidl(pidl);
        if reveal {
            // cidl=0 selects this exact file inside its parent. No Explorer command-line parsing.
            SHOpenFolderAndSelectItems(pidl.0, None, 0).context("资源管理器无法定位此文件")?;
        } else {
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_IDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
                lpIDList: pidl.0.cast(),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            // Wait for dispatch only, never wait for the application to exit or become idle.
            ShellExecuteExW(&mut info).context("系统无法打开此文件，请检查默认打开应用")?;
        }
        Ok(())
    })
}

#[derive(Default)]
struct DeleteOutcome {
    rejected: bool,
    result: Option<(HRESULT, bool)>,
}
#[implement(IFileOperationProgressSink)]
struct RecycleSink(Arc<Mutex<DeleteOutcome>>);
#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for RecycleSink_Impl {
    fn PreDeleteItem(&self, flags: u32, _: Ref<IShellItem>) -> windows::core::Result<()> {
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            self.0.lock().unwrap().rejected = true;
            return Err(windows::core::Error::new(
                E_ABORT,
                "此位置不支持回收站，已阻止永久删除",
            ));
        }
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        result: HRESULT,
        recycled: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.0.lock().unwrap().result = Some((result, !recycled.is_null()));
        Ok(())
    }
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _hrresult: windows::core::HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(
        &self,
        _dwflags: u32,
        _psiitem: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _dwflags: u32,
        _psiitem: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
        _hrrename: windows::core::HRESULT,
        _psinewlycreated: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _dwflags: u32,
        _psiitem: windows::core::Ref<IShellItem>,
        _psidestinationfolder: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _dwflags: u32,
        _psiitem: windows::core::Ref<IShellItem>,
        _psidestinationfolder: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
        _hrmove: windows::core::HRESULT,
        _psinewlycreated: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _dwflags: u32,
        _psiitem: windows::core::Ref<IShellItem>,
        _psidestinationfolder: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _dwflags: u32,
        _psiitem: windows::core::Ref<IShellItem>,
        _psidestinationfolder: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
        _hrcopy: windows::core::HRESULT,
        _psinewlycreated: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(
        &self,
        _dwflags: u32,
        _psidestinationfolder: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _dwflags: u32,
        _psidestinationfolder: windows::core::Ref<IShellItem>,
        _psznewname: &windows::core::PCWSTR,
        _psztemplatename: &windows::core::PCWSTR,
        _dwfileattributes: u32,
        _hrnew: windows::core::HRESULT,
        _psinewitem: windows::core::Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _iworktotal: u32, _iworksofar: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

pub(crate) fn recycle(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    in_apartment(move || unsafe {
        let item = shell_item(&path)?;
        let operation: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
        operation.SetOperationFlags(
            FOF_NO_UI
                | FOF_ALLOWUNDO
                | FOF_NO_CONNECTED_ELEMENTS
                | FOF_WANTNUKEWARNING
                | FOFX_RECYCLEONDELETE
                | FOFX_ADDUNDORECORD
                | FOFX_EARLYFAILURE,
        )?;
        let outcome = Arc::new(Mutex::new(DeleteOutcome::default()));
        let sink: IFileOperationProgressSink = RecycleSink(outcome.clone()).into();
        operation.DeleteItem(&item, &sink)?;
        let performed = operation.PerformOperations();
        let outcome = outcome.lock().unwrap();
        if outcome.rejected {
            bail!("此位置不支持回收站，文件已保留，未执行永久删除。");
        }
        if let Some((result, recycled)) = outcome.result {
            result.ok().context("Windows 回收站处理失败")?;
            if !recycled {
                bail!("Windows 没有返回回收站中的文件，请核对源路径与回收站。");
            }
        } else {
            performed.context("Windows 回收站操作未完成")?;
            bail!("系统未执行回收站操作，可能是磁盘不支持回收站、空间不足或权限不足；请在资源管理器中检查。");
        }
        performed.context("Windows 回收站操作失败")?;
        if operation.GetAnyOperationsAborted()?.as_bool() {
            bail!("系统中止了回收站操作，请核对该文件。");
        }
        Ok(())
    })
}
