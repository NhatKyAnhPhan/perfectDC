//! Tiện ích hệ thống Windows: quyền admin, chạy PowerShell ẩn, hộp thoại, log.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW, SW_SHOWNORMAL,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Ứng dụng có đang chạy với quyền Administrator?
pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// Khởi chạy lại chính ứng dụng với quyền Administrator (UAC).
pub fn relaunch_as_admin() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Không lấy được đường dẫn exe: {e}"))?;
    let exe_w = wide(&exe.to_string_lossy());
    let verb = wide("runas");
    let params: String = std::env::args_os()
        .skip(1)
        .map(|a| format!("\"{}\"", a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    let params_w = wide(&params);
    let ret = unsafe {
        ShellExecuteW(
            0,
            verb.as_ptr(),
            exe_w.as_ptr(),
            params_w.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Trả về > 32 nghĩa là thành công.
    if ret as usize > 32 {
        Ok(())
    } else {
        Err(format!(
            "Không thể yêu cầu quyền Administrator (mã {}). Hãy chuột phải → Run as administrator.",
            ret as usize
        ))
    }
}

/// Chạy PowerShell ẩn cửa sổ, trả về stdout. Lỗi kèm stderr để dễ chẩn đoán.
pub fn powershell(script: &str) -> Result<String, String> {
    let full = format!(
        "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;$OutputEncoding=[System.Text.Encoding]::UTF8;{script}"
    );
    let mut utf16 = Vec::with_capacity(full.len() * 2);
    for u in full.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);

    let out = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Không chạy được PowerShell: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout)
        .trim_start_matches('\u{feff}')
        .to_string();
    let stderr = String::from_utf8_lossy(&out.stderr)
        .trim_start_matches('\u{feff}')
        .to_string();
    if !out.status.success() {
        let mut msg = stderr.trim().to_string();
        if msg.is_empty() {
            msg = stdout.trim().to_string();
        }
        if msg.is_empty() {
            msg = format!("PowerShell kết thúc với mã {:?}", out.status.code());
        }
        return Err(clean_ps_error(&msg));
    }
    Ok(stdout.trim().to_string())
}

/// Rút gọn thông báo lỗi PowerShell (bỏ stack trace dài dòng).
fn clean_ps_error(s: &str) -> String {
    let mut lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // Ưu tiên dòng bắt đầu bằng "Exception"/"Lỗi" hoặc dòng đầu tiên.
    if let Some(pos) = lines
        .iter()
        .position(|l| l.starts_with("Exception") || l.contains("Error"))
    {
        lines.truncate(pos + 1);
    } else if lines.len() > 8 {
        lines.truncate(8);
    }
    lines.join("\n")
}

/// Mở một đường dẫn/thư mục bằng Windows Explorer.
pub fn open_path(path: &Path) -> Result<(), String> {
    let target = wide(&path.to_string_lossy());
    let verb = wide("open");
    let ret = unsafe {
        ShellExecuteW(
            0,
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if ret as usize > 32 {
        Ok(())
    } else {
        Err("Không mở được đường dẫn.".to_string())
    }
}

pub fn message_box_error(title: &str, text: &str) {
    message_box(title, text, MB_ICONERROR);
}

pub fn message_box_info(title: &str, text: &str) {
    message_box(title, text, MB_ICONINFORMATION);
}

fn message_box(title: &str, text: &str, icon: u32) {
    let t = wide(title);
    let m = wide(text);
    unsafe {
        MessageBoxW(0, m.as_ptr(), t.as_ptr(), MB_OK | icon);
    }
}

/// Thư mục lưu log: %LOCALAPPDATA%\PerfectDS
pub fn log_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("PerfectDS");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn log_file() -> PathBuf {
    log_dir().join("operations.log")
}

/// Thời gian địa phương dạng `YYYY-MM-DD HH:MM:SS`.
pub fn timestamp() -> String {
    let mut st = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// Ghi một dòng vào log thao tác (không làm ứng dụng lỗi nếu ghi thất bại).
pub fn append_log(level: &str, message: &str) {
    let path = log_file();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let line = format!("[{}] [{}] {}\n", timestamp(), level, message);
        let _ = f.write_all(line.as_bytes());
    }
}

/// Bọc chuỗi thành literal PowerShell an toàn (nháy đơn, nhân đôi nháy đơn).
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
