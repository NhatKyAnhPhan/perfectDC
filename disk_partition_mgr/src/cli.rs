//! Module phân tích tham số dòng lệnh và dispatch lệnh.

use crate::disk_info::{self, DiskInfo, Mode};
use crate::planner::{self, Direction, Plan};
use crate::sys;

pub fn run(args: Vec<String>) -> Result<(), String> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return Ok(());
    }
    match args.first().map(|s| s.as_str()) {
        Some("inspect") => cmd_inspect(&args[1..]),
        Some("plan-move") => cmd_plan_move(&args[1..]),
        Some("--version") | Some("-v") => {
            println!("PerfectDS {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => {
            print_usage();
            Err(format!("Không nhận diện lệnh: {other}"))
        }
        None => {
            print_usage();
            Err("Thiếu lệnh.".to_string())
        }
    }
}

fn print_usage() {
    println!(
        r#"PerfectDS — kiểm tra và mô phỏng thao tác quản lý phân vùng đĩa (Windows)

⚠️  Mặc định luôn là dry-run: không ghi đĩa thật trừ khi truyền --apply.
   Đọc đĩa vật lý cần chạy bằng quyền Administrator.

Lệnh:
  inspect [--disk <đường_dẫn>]   Hiển thị thông tin đĩa (kích thước, sector, phân vùng, unallocated);
                                hoặc liệt kê các đĩa vật lý khi không có --disk
  plan-move --disk <đường_dẫn> --partition <n> --direction <left|right>
            --offset-sectors <n> [--dry-run] [--apply]
                                Tính toán và in kế hoạch dời phân vùng

Tham số:
  --disk            Đĩa vật lý (\\.\PhysicalDriveN) hoặc file ảnh (.vhd)
  --partition       Số thứ tự phân vùng (theo số hiển thị trong inspect)
  --direction       left: dời về phía đầu đĩa; right: về phía cuối đĩa
  --offset-sectors  Số sector muốn dịch chuyển (0 = không dịch)
  --apply           Cho phép ghi dữ liệu (mặc định dry-run)
  --dry-run         Chỉ in kế hoạch (mặc định)

Ví dụ:
  dpm-cli.exe inspect
  dpm-cli.exe inspect --disk \\?\C:\test\disk.vhd
  dpm-cli.exe plan-move --disk \\.\PhysicalDrive0 --partition 2 --direction right --offset-sectors 16 --dry-run"#
    );
}

#[derive(Default)]
struct Opts {
    opts: Vec<(String, String)>,
    flags: Vec<String>,
    positionals: Vec<String>,
}

impl Opts {
    fn opt(&self, key: &str) -> Option<&str> {
        self.opts
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    fn has_flag(&self, key: &str) -> bool {
        self.flags.iter().any(|f| f == key)
    }
    fn opt_u64(&self, key: &str) -> Option<u64> {
        self.opt(key).and_then(|v| v.parse::<u64>().ok())
    }
    fn opt_u32(&self, key: &str) -> Option<u32> {
        self.opt(key).and_then(|v| v.parse::<u32>().ok())
    }
}

fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(k) = a.strip_prefix("--") {
            let (k, inline) = match k.find('=') {
                Some(pos) => (&k[..pos], Some(k[pos + 1..].to_string())),
                None => (k, None),
            };
            let has_inline = inline.is_some();
            if matches!(k, "disk" | "partition" | "direction" | "offset-sectors") {
                let v = match inline {
                    Some(v) => v,
                    None => args
                        .get(i + 1)
                        .cloned()
                        .ok_or_else(|| format!("--{k} cần giá trị"))?,
                };
                o.opts.push((k.to_string(), v));
                i += if has_inline { 1 } else { 2 };
            } else {
                o.flags.push(k.to_string());
                i += 1;
            }
        } else {
            o.positionals.push(a.clone());
            i += 1;
        }
    }
    Ok(o)
}

fn cmd_inspect(args: &[String]) -> Result<(), String> {
    let o = parse_args(args)?;
    match o.opt("disk") {
        Some(path) => {
            let info = disk_info::read_disk(path, Mode::Read)?;
            print_disk_info(&info);
        }
        None => {
            let drives = disk_info::list_physical_drives();
            if drives.is_empty() {
                println!("Không tìm thấy đĩa vật lý nào.");
                if sys::is_admin() {
                    println!("Gợi ý: dùng --disk <đường_dẫn> để đọc file ảnh (.vhd).");
                } else {
                    println!("Gợi ý: hãy chạy lại bằng quyền Administrator để đọc đĩa vật lý.");
                }
                return Ok(());
            }
            println!(
                "Đĩa vật lý khả dụng: {}",
                drives
                    .iter()
                    .map(|i| disk_info::physical_drive_path(*i))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for i in drives {
                match disk_info::read_disk(&disk_info::physical_drive_path(i), Mode::Read) {
                    Ok(info) => println!(
                        "  {} — {} ({})",
                        disk_info::physical_drive_path(i),
                        human_size(info.size_bytes),
                        info.style.as_str()
                    ),
                    Err(e) => println!("  {} — Lỗi: {e}", disk_info::physical_drive_path(i)),
                }
            }
        }
    }
    Ok(())
}

fn print_disk_info(info: &DiskInfo) {
    println!("Đĩa:            {}", info.path);
    println!("Loại:           {}", info.style.as_str());
    println!(
        "Kích thước:     {} ({} bytes)",
        human_size(info.size_bytes),
        info.size_bytes
    );
    println!("Sector size:    {} B", info.sector_size);
    if info.partitions.is_empty() {
        println!("Phân vùng:      (không tìm thấy phân vùng nào)");
        return;
    }
    println!("Phân vùng:");
    println!("  {:<3} {:<20} {:<20} Mô tả", "#", "Offset", "Độ dài");
    for p in &info.partitions {
        println!(
            "  {:<3} 0x{:016X}  0x{:016X}  {}",
            p.number,
            p.start_offset,
            p.length,
            p.describe()
        );
    }
    println!("Vùng chưa phân bổ (unallocated):");
    for (s, e) in info.gaps() {
        if e > s {
            println!(
                "  [0x{:016X}, 0x{:016X}) — độ dài 0x{:X} ({})",
                s,
                e,
                e - s,
                human_size(e - s)
            );
        }
    }
}

fn cmd_plan_move(args: &[String]) -> Result<(), String> {
    let o = parse_args(args)?;
    let disk_path = o
        .opt("disk")
        .ok_or("Thiếu --disk (đĩa vật lý hoặc file ảnh .vhd)")?
        .to_string();
    let partition = o.opt_u32("partition").ok_or("Thiếu --partition")?;
    let direction = parse_direction(o.opt("direction").ok_or("Thiếu --direction (left|right)")?)?;
    let offset_sectors = o.opt_u64("offset-sectors").unwrap_or(0);
    let apply = o.has_flag("apply");
    let mode = if apply { Mode::ReadWrite } else { Mode::Read };
    let disk = disk_info::read_disk(&disk_path, mode)?;
    let plan =
        planner::plan_move(&disk, partition, direction, offset_sectors).map_err(|e| e.message())?;
    print_plan(&plan);
    if apply {
        if disk_info::is_physical_drive_path(&disk_path) {
            println!(
                "⚠️  Cảnh báo: đang thao tác --apply trên ĐĨA VẬT LÝ ({}) — ghi dữ liệu thực tế trên ổ đĩa.",
                disk_path
            );
        }
        let copied = planner::execute_move(&plan)?;
        println!("Đã copy {copied} bytes (--apply): dữ liệu đã được di chuyển.");
    } else {
        println!("(dry-run) Chưa ghi đĩa — nếu muốn ghi, thêm --apply.");
    }
    Ok(())
}

fn print_plan(plan: &Plan) {
    let sector = plan.sector_size;
    println!(
        "Kế hoạch dời phân vùng #{} theo hướng {}",
        plan.partition_number,
        plan.direction.as_str()
    );
    println!("  Đĩa:              {}", plan.disk_path);
    println!(
        "  Phân vùng:        [0x{:016X}, 0x{:016X})",
        plan.old_start,
        plan.old_start + plan.old_length
    );
    println!(
        "  Mục tiêu (mới):   [0x{:016X}, 0x{:016X})",
        plan.new_start,
        plan.new_start + plan.old_length
    );
    println!(
        "  Delta:            {} bytes ({} sector{})",
        plan.delta,
        plan.delta / sector as i64,
        if (plan.delta / sector as i64) == 1 {
            ""
        } else {
            "s"
        }
    );
    println!("  Căn chỉnh:        new_start căn chỉnh theo sector size ({sector} B)");
    println!(
        "  Bước copy ({} bước, mỗi bước tối đa {} bytes):",
        plan.copy_steps.len(),
        planner::chunk_size(sector)
    );
    for step in &plan.copy_steps {
        println!(
            "    Bước {:>2}: read  0x{:016X} len 0x{:X}",
            step.index, step.src_offset, step.size
        );
        println!(
            "                 write 0x{:016X} len 0x{:X}",
            step.dst_offset, step.size
        );
    }
    println!(
        "  Kiểm tra va chạm: không phát hiện phân vùng nào nằm trong mục tiêu [0x{:016X}, 0x{:016X}).",
        plan.new_start,
        plan.new_start + plan.old_length
    );
    match plan.gap {
        Some((s, e)) => println!(
            "  Vùng trống:       đích nằm trong unallocated [0x{s:016X}, 0x{e:016X}) — độ dài 0x{:X} ({})",
            e - s,
            human_size(e - s)
        ),
        None => println!(
            "  Vùng trống:       đích không nằm trong unallocated (đã kiểm tra trùng phạm vi với các phân vùng khác)."
        ),
    }
    if !plan.warnings.is_empty() {
        println!("  Cảnh báo:");
        for w in &plan.warnings {
            println!("    - {w}");
        }
    }
}

fn parse_direction(s: &str) -> Result<Direction, String> {
    match s.to_ascii_lowercase().as_str() {
        "left" | "l" => Ok(Direction::Left),
        "right" | "r" => Ok(Direction::Right),
        other => Err(format!(
            "Tham số --direction không hợp lệ: {other:?} (cần left hoặc right)"
        )),
    }
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n}");
    }
    let mut x = n as f64;
    let mut unit = 0;
    while x >= 1024.0 && unit + 1 < UNITS.len() {
        x /= 1024.0;
        unit += 1;
    }
    format!("{x:.2} {}", UNITS[unit])
}
