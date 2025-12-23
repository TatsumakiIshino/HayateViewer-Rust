use std::path::PathBuf;
use windows::{
    Win32::Foundation::*, Win32::System::Com::*, Win32::UI::Shell::Common::*, Win32::UI::Shell::*,
    core::*,
};

pub fn select_folder(parent: HWND) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;

        dialog.SetOptions(FOS_PICKFOLDERS).ok()?;

        if dialog.Show(Some(parent)).is_err() {
            return None;
        }

        let result = dialog.GetResult().ok()?;
        let path_pwstr = result.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = path_pwstr.to_string().ok()?;
        CoTaskMemFree(Some(path_pwstr.as_ptr() as *const _));

        Some(PathBuf::from(path))
    }
}

pub fn select_archive_file(parent: HWND) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;

        let filter = [
            COMDLG_FILTERSPEC {
                pszName: w!("Supported Archives"),
                pszSpec: w!("*.zip;*.7z;*.cbz;*.rar;*.cbr"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("All Files"),
                pszSpec: w!("*.*"),
            },
        ];

        dialog.SetFileTypes(&filter).ok()?;

        if dialog.Show(Some(parent)).is_err() {
            return None;
        }

        let result = dialog.GetResult().ok()?;
        let path_pwstr = result.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = path_pwstr.to_string().ok()?;
        CoTaskMemFree(Some(path_pwstr.as_ptr() as *const _));

        Some(PathBuf::from(path))
    }
}
