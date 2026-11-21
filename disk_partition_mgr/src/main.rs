//! `disk_partition_mgr` — CLI kiểm tra và mô phỏng thao tác quản lý phân vùng đĩa (Windows).
//!
//! Mặc định luôn là chế độ dry-run: tuyệt đối không ghi trực tiếp đĩa thật trừ khi
//! được truyền `--apply` (thực hiện sau khi kiểm tra va chạm/biên giới).

mod cli;
mod disk_info;
mod planner;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = cli::run(args) {
        eprintln!("Lỗi: {e}");
        std::process::exit(1);
    }
}
