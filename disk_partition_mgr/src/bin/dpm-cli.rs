//! CLI tương thích: `dpm-cli inspect ...` / `dpm-cli plan-move ...`.

fn main() {
    // Hiển thị đúng tiếng Việt trong Windows Terminal / cmd.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = perfectds::cli::run(args) {
        eprintln!("Lỗi: {e}");
        std::process::exit(1);
    }
}
