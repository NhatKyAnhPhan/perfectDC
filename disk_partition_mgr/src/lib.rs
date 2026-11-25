//! PerfectDS — thư viện lõi quản lý ổ đĩa/phân vùng trên Windows.
//!
//! - [`disk_info`]: đọc trực tiếp bảng phân vùng MBR/GPT qua Win32 IOCTL (không cần admin).
//! - [`planner`]: lập kế hoạch dời phân vùng ở mức sector (dùng cho CLI nâng cao).
//! - [`model`]: mô hình dữ liệu đĩa/phân vùng cho GUI.
//! - [`sys`]: gọi PowerShell ẩn, kiểm tra quyền admin, ghi log thao tác.
//! - [`ops`]: các thao tác chia/gộp/xoá/định dạng ổ đĩa có kiểm tra an toàn.

pub mod cli;
pub mod disk_info;
pub mod gui;
pub mod model;
pub mod ops;
pub mod planner;
pub mod sys;
