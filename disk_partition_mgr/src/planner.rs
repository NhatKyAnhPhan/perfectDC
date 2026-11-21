//! Module tính toán kế hoạch dời phân vùng:
//! căn chỉnh offset/sector, phát hiện va chạm/biên giới, xây dựng các bước copy.

use crate::disk_info::{close_disk, copy_range_h, DiskInfo, Mode, open_disk, PartitionInfo, read_disk};

/// Hướng dời phân vùng: `left` = về phía đầu đĩa (offset giảm), `right` = về phía cuối đĩa (offset tăng).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Left,
    Right,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Left => "left",
            Direction::Right => "right",
        }
    }
}

/// Một bước copy dữ liệu: `Read` tại `src_offset` → `Write` tại `dst_offset` (căn chỉnh theo sector).
#[derive(Clone, Copy, Debug)]
pub struct CopyStep {
    pub index: u32,
    pub src_offset: u64,
    pub dst_offset: u64,
    pub size: u64,
}

/// Kế hoạch dời một phân vùng.
#[derive(Clone, Debug)]
pub struct Plan {
    pub disk_path: String,
    pub partition_number: u32,
    pub sector_size: u64,
    pub old_start: u64,
    pub old_length: u64,
    pub new_start: u64,
    pub delta: i64,
    pub direction: Direction,
    pub offset_sectors: u64,
    pub copy_steps: Vec<CopyStep>,
    pub gap: Option<(u64, u64)>,
    pub warnings: Vec<String>,
}

/// Lỗi khi tính kế hoạch.
#[derive(Debug)]
pub enum PlanError {
    PartitionNotFound {
        number: u32,
        available: Vec<u32>,
    },
    OutOfBounds {
        target_start: u64,
        length: u64,
        disk_size: u64,
    },
    Collision {
        overlaps: Vec<u32>,
    },
}

impl PlanError {
    pub fn message(&self) -> String {
        match self {
            PlanError::PartitionNotFound { number, available } => format!(
                "Không tìm thấy phân vùng #{number}. Phân vùng tồn tại: {}.",
                if available.is_empty() {
                    "(không có)".to_string()
                } else {
                    available
                        .iter()
                        .map(|n| format!("#{n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            PlanError::OutOfBounds {
                target_start,
                length,
                disk_size,
            } => format!(
                "Mục tiêu [0x{target_start:X}, +0x{length:X}) vượt ra ngoài đĩa (kích thước đĩa 0x{disk_size:X})."
            ),
            PlanError::Collision { overlaps } => format!(
                "Va chạm phân vùng: {} — đích trùng phạm vi của phân vùng này.",
                overlaps
                    .iter()
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// Căn chỉnh lên gần nhất (`n == 0` → 0).
pub fn align_up(n: u64, align: u64) -> u64 {
    if align == 0 || n == 0 {
        return n;
    }
    n.div_ceil(align) * align
}

/// Căn chỉnh xuống gần nhất.
pub fn align_down(n: u64, align: u64) -> u64 {
    if align == 0 {
        return n;
    }
    n / align * align
}

/// Kích thước block copy mặc định (căn chỉnh theo sector, tối đa 1 MiB).
pub fn chunk_size(sector: u64) -> u64 {
    let s = sector.max(512);
    s.saturating_mul(32).min(1024 * 1024)
}

/// Danh sách các phân vùng trùng với `[start, end)` (loại trừ phân vùng `exclude`).
pub fn find_overlaps(disk: &DiskInfo, start: u64, end: u64, exclude: u32) -> Vec<u32> {
    disk.partitions
        .iter()
        .filter(|p| {
            p.number != exclude
                && p.length > 0
                && p.start_offset < end
                && start < p.start_offset.saturating_add(p.length)
        })
        .map(|p| p.number)
        .collect()
}

/// Tính kế hoạch dời phân vùng `partition_number` theo `direction` bởi `offset_sectors` sector.
///
/// - right: mục tiêu tăng (dịch về phía cuối đĩa).
/// - left: mục tiêu giảm (dịch về phía đầu đĩa).
/// - Căn chỉnh: `new_start` luôn là bội của sector size; va chạm với phân vùng nào đó (trừ
///   phân vùng đang dời) → `PlanError::Collision`.
pub fn plan_move(
    disk: &DiskInfo,
    partition_number: u32,
    direction: Direction,
    offset_sectors: u64,
) -> Result<Plan, PlanError> {
    let sector = disk.sector_size.max(1) as u64;
    let p = disk.partition_by_number(partition_number).ok_or(PlanError::PartitionNotFound {
        number: partition_number,
        available: disk.partitions.iter().map(|x| x.number).collect(),
    })?;
    let abs_delta = offset_sectors.saturating_mul(sector);
    let signed = abs_delta.min(i64::MAX as u64) as i64;
    let raw_start = p.start_offset as i64
        + if direction == Direction::Right {
            signed
        } else {
            -signed
        };
    // Căn chỉnh theo sector size: mục tiêu luôn là bội của sector size.
    let new_start = match direction {
        Direction::Right => align_up(raw_start as u64, sector),
        Direction::Left => align_down(raw_start as u64, sector),
    };
    let delta = new_start as i64 - p.start_offset as i64;
    let new_end = new_start.checked_add(p.length).ok_or(PlanError::OutOfBounds {
        target_start: new_start,
        length: p.length,
        disk_size: disk.size_bytes,
    })?;
    if new_end > disk.size_bytes {
        return Err(PlanError::OutOfBounds {
            target_start: new_start,
            length: p.length,
            disk_size: disk.size_bytes,
        });
    }
    let overlaps = find_overlaps(disk, new_start, new_end, p.number);
    if !overlaps.is_empty() {
        return Err(PlanError::Collision { overlaps });
    }
    let gap = disk
        .gaps()
        .into_iter()
        .find(|(s, e)| new_start >= *s && new_start < *e);
    let copy_steps = build_steps(p, new_start, direction, sector);
    let mut warnings: Vec<String> = Vec::new();
    if p.is_encrypted {
        warnings.push(format!(
            "Phân vùng #{n} đang bật BitLocker (volume đã mã hóa) — copy dữ liệu trên phân vùng đã mã hóa thường không an toàn và BitLocker có thể bị vô hiệu hóa.",
            n = p.number
        ));
    }
    if p.is_system {
        warnings.push(format!(
            "Phân vùng #{n} chứa hệ thống (ESP/boot volume) — thao tác này có thể phá vỡ khả năng khởi động.",
            n = p.number
        ));
    }
    if offset_sectors == 0 {
        warnings.push("offset_sectors = 0 — không có gì để di chuyển (0 bước copy).".to_string());
    }
    Ok(Plan {
        disk_path: disk.path.clone(),
        partition_number,
        sector_size: sector,
        old_start: p.start_offset,
        old_length: p.length,
        new_start,
        delta,
        direction,
        offset_sectors,
        copy_steps,
        gap,
        warnings,
    })
}

/// Xây dựng danh sách bước copy (đã căn chỉnh thứ tự để tránh đè dữ liệu chưa đọc):
/// - right: copy từ cuối về đầu (`dst > src`, ghi ở trên các reads chưa tới).
/// - left: copy từ đầu về cuối (`dst < src`, ghi ở dưới các reads tiếp theo).
fn build_steps(p: &PartitionInfo, new_start: u64, direction: Direction, sector: u64) -> Vec<CopyStep> {
    let chunk = chunk_size(sector);
    let total_blocks = p.length.div_ceil(chunk);
    let mut steps = Vec::with_capacity(total_blocks.min(1 << 20) as usize);
    let mut index = 1u32;
    match direction {
        Direction::Right => {
            let mut i = total_blocks;
            while i > 0 {
                i -= 1;
                let off = i * chunk;
                steps.push(CopyStep {
                    index,
                    src_offset: p.start_offset + off,
                    dst_offset: new_start + off,
                    size: (p.length - off).min(chunk),
                });
                index += 1;
            }
        }
        Direction::Left => {
            for i in 0..total_blocks {
                let off = i * chunk;
                steps.push(CopyStep {
                    index,
                    src_offset: p.start_offset + off,
                    dst_offset: new_start + off,
                    size: (p.length - off).min(chunk),
                });
                index += 1;
            }
        }
    }
    steps
}

/// Thực thi kế hoạch: re-validate (đọc lại đĩa, kiểm tra va chạm/biên giới) rồi copy theo các bước.
/// Trả về số bytes đã copy. Nếu trạng thái đĩa đã thay đổi kể từ khi lập kế hoạch → lỗi, không ghi.
pub fn execute_move(plan: &Plan) -> Result<u64, String> {
    let disk = read_disk(&plan.disk_path, Mode::ReadWrite)?;
    let recomputed = plan_move(&disk, plan.partition_number, plan.direction, plan.offset_sectors)
        .map_err(|e| e.message())?;
    if recomputed.new_start != plan.new_start {
        return Err(
            "Trạng thái đĩa đã thay đổi kể từ khi lập kế hoạch — hủy thao tác để an toàn."
                .to_string(),
        );
    }
    let h = open_disk(&plan.disk_path, Mode::ReadWrite)?;
    let mut copied = 0u64;
    for step in &plan.copy_steps {
        let n = copy_range_h(h, step.src_offset, step.dst_offset, step.size)?;
        copied += n;
        println!(
            "  Bước {}/{}: read  0x{:016X} len 0x{:X}",
            step.index,
            plan.copy_steps.len(),
            step.src_offset,
            step.size
        );
        println!("                 write 0x{:016X} len 0x{:X}", step.dst_offset, step.size);
    }
    close_disk(h);
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk_info::{DiskInfo, PartitionInfo, Style};

    /// Đĩa test: size 0x60000; phân vùng #1 [0x20000, 0x30000), #2 [0x40000, 0x48000).
    /// Vùng unallocated: [0x30000, 0x40000) và [0x48000, 0x60000).
    fn test_disk() -> DiskInfo {
        fn part(number: u32, start: u64, len: u64) -> PartitionInfo {
            PartitionInfo {
                number,
                style: Style::Mbr,
                start_offset: start,
                length: len,
                mbr_type: 7,
                boot_indicator: false,
                recognized_partition: true,
                gpt_type_guid: None,
                gpt_attributes: 0,
                gpt_name: String::new(),
                is_system: false,
                is_encrypted: false,
            }
        }
        DiskInfo {
            path: "test-disk".into(),
            size_bytes: 0x60000,
            sector_size: 512,
            style: Style::Mbr,
            partitions: vec![part(1, 0x20000, 0x10000), part(2, 0x40000, 0x8000)],
        }
    }

    #[test]
    fn test_align_up_down() {
        assert_eq!(align_up(0, 512), 0);
        assert_eq!(align_up(1, 512), 512);
        assert_eq!(align_up(511, 512), 512);
        assert_eq!(align_up(512, 512), 512);
        assert_eq!(align_up(513, 512), 1024);
        assert_eq!(align_up(1023, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
        assert_eq!(align_down(1023, 512), 512);
        assert_eq!(align_down(512, 512), 512);
        assert_eq!(align_down(513, 512), 512);
        assert_eq!(align_down(4095, 4096), 0);
        assert_eq!(align_down(4096, 4096), 4096);
        // align == 0 → trả về n
        assert_eq!(align_up(7, 0), 7);
        assert_eq!(align_down(7, 0), 7);
    }

    #[test]
    fn test_find_overlaps() {
        let d = test_disk();
        // Không trùng gì: [0x22000, 0x24000) nằm trong gap [0x30000..)? hmm — 0x22000+0x2000 = 0x24000; A = [0x20000, 0x30000) — excluded anyway.
        // Range chỉ nằm trong gap [0x30000, 0x40000): [0x30200, 0x30400) — không trùng ai
        assert!(find_overlaps(&d, 0x30200, 0x30400, 1).is_empty());
        assert!(find_overlaps(&d, 0x30200, 0x30400, 2).is_empty());
        // Range chạm đúng ranh B_end và B_start: [0x3FFF0, 0x40000) — end == B.start → không overlap
        assert!(find_overlaps(&d, 0x3FFF0, 0x40000, 1).is_empty());
        // Range nằm ngay sau B: [0x48000, 0x49000) — start == B_end → không overlap
        assert!(find_overlaps(&d, 0x48000, 0x49000, 1).is_empty());
        // Range trùng A [0x20000, 0x30000): [0x21000, 0x22000)
        assert_eq!(find_overlaps(&d, 0x21000, 0x22000, 2), vec![1]);
        assert!(find_overlaps(&d, 0x21000, 0x22000, 1).is_empty());
        // Range trùng B [0x40000, 0x48000): [0x41000, 0x42000)
        assert_eq!(find_overlaps(&d, 0x41000, 0x42000, 1), vec![2]);
        assert!(find_overlaps(&d, 0x41000, 0x42000, 2).is_empty());
        // Range trùng B với end > B end: [0x42000, 0x49000) → overlaps B
        assert_eq!(find_overlaps(&d, 0x42000, 0x49000, 1), vec![2]);
    }

    #[test]
    fn test_plan_move_right() {
        let d = test_disk();
        // #1 [0x20000, 0x30000), dịch phải 4 sector (512) → new_start = 0x20800, delta = +0x800.
        let plan = plan_move(&d, 1, Direction::Right, 4).unwrap();
        assert_eq!(plan.new_start, 0x20000 + 4 * 512);
        assert_eq!(plan.delta, 0x800);
        assert_eq!(plan.old_start, 0x20000);
        assert_eq!(plan.old_length, 0x10000);
        // chunk = 512*32 = 16384 → 4 blocks
        assert_eq!(plan.copy_steps.len(), 4);
        // Right: copy từ cuối về đầu (grav bằng dst > src).
        let first = &plan.copy_steps[0];
        assert_eq!(first.index, 1);
        assert_eq!(first.src_offset, 0x20000 + 3 * 0x4000);
        assert_eq!(first.dst_offset, 0x20800 + 3 * 0x4000);
        assert_eq!(first.size, 0x4000);
        let last = &plan.copy_steps[3];
        assert_eq!(last.src_offset, 0x20000);
        assert_eq!(last.dst_offset, 0x20800);
    }

    #[test]
    fn test_plan_move_left() {
        let d = test_disk();
        // #2 [0x40000, 0x48000), dịch trái 4 sector → new_start = 0x3F800, delta = -0x800.
        let plan = plan_move(&d, 2, Direction::Left, 4).unwrap();
        assert_eq!(plan.new_start, 0x40000 - 4 * 512);
        assert_eq!(plan.delta, -(4 * 512));
        assert_eq!(plan.copy_steps.len(), 2); // 0x8000 = 2 * 0x4000
        // Left: copy từ đầu về cuối (grav bằng dst < src).
        assert_eq!(plan.copy_steps[0].src_offset, 0x40000);
        assert_eq!(plan.copy_steps[0].dst_offset, 0x3F800);
        assert_eq!(plan.copy_steps[1].src_offset, 0x40000 + 0x4000);
        assert_eq!(plan.copy_steps[1].dst_offset, 0x3F800 + 0x4000);
    }

    #[test]
    fn test_plan_move_gap() {
        let d = test_disk();
        // new_start = 0x30000 (là đầu gap [0x30000, 0x40000)) → 128 sectors * 512 = 0x10000 → 0x20000 + 0x10000 = 0x30000
        let plan = plan_move(&d, 1, Direction::Right, 128).unwrap();
        assert_eq!(plan.new_start, 0x30000);
        assert_eq!(plan.gap, Some((0x30000, 0x40000)));
    }

    #[test]
    fn test_plan_move_collision() {
        let d = test_disk();
        // new_start = 0x20000 + 129*512 = 0x30080, new_end = 0x40080 > B.start 0x40000 → va chạm B.
        match plan_move(&d, 1, Direction::Right, 129) {
            Err(PlanError::Collision { overlaps }) => assert_eq!(overlaps, vec![2]),
            other => panic!("Expect collision, got {other:?}"),
        }
    }

    #[test]
    fn test_plan_move_out_of_bounds() {
        let d = test_disk();
        // right từ #1: new_start = 0x20000 + 0x200*512? 0x200 = 512 → 0x20000 + 0x40000 = 0x60000 → dst = [0x60000, 0x70000) > size.
        assert!(matches!(
            plan_move(&d, 1, Direction::Right, 0x200),
            Err(PlanError::OutOfBounds { .. })
        ));
        // left từ #1: new_start < 0.
        assert!(matches!(
            plan_move(&d, 1, Direction::Left, 0x200),
            Err(PlanError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn test_plan_move_not_found() {
        let d = test_disk();
        match plan_move(&d, 99, Direction::Right, 1) {
            Err(PlanError::PartitionNotFound { number, available }) => {
                assert_eq!(number, 99);
                assert_eq!(available, vec![1, 2]);
            }
            other => panic!("Expect not found, got {other:?}"),
        }
    }

    #[test]
    fn test_plan_move_zero_offset_warns() {
        let d = test_disk();
        let plan = plan_move(&d, 1, Direction::Right, 0).unwrap();
        assert_eq!(plan.delta, 0);
        assert!(plan.warnings.iter().any(|w| w.contains("offset_sectors = 0")));
    }
}
