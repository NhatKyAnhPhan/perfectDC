//! PerfectDS — ứng dụng GUI quản lý & chia ổ đĩa trên Windows.
//!
//! Bản GUI luôn mở giao diện. Dùng `dpm-cli.exe` cho các lệnh dòng lệnh
//! (`inspect`, `plan-move`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(e) = perfectds::gui::run() {
        perfectds::sys::message_box_error("PerfectDS", &format!("Không mở được giao diện: {e}"));
        std::process::exit(1);
    }
}
