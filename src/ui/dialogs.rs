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

pub fn show_confirm_dialog(parent: HWND, title: &str, message: &str) -> bool {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW,
        };

        let mut title_wide: Vec<u16> = title.encode_utf16().collect();
        title_wide.push(0);
        let mut message_wide: Vec<u16> = message.encode_utf16().collect();
        message_wide.push(0);

        let result = MessageBoxW(
            Some(parent),
            PCWSTR(message_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        );

        result == IDYES
    }
}
