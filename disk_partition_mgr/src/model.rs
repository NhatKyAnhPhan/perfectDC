//! Mô hình dữ liệu đĩa/phân vùng dùng cho GUI (nhận từ Windows Storage API).

use serde::Deserialize;
use serde_json::Value;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// Một đĩa vật lý.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiskDto {
    pub number: u32,
    pub name: String,
    pub serial: String,
    pub size: u64,
    /// "GPT" | "MBR" | "RAW"
    pub style: String,
    pub bus_type: String,
    pub health: String,
    pub operational: String,
    pub is_boot: bool,
    pub is_system: bool,
    pub is_read_only: bool,
    pub is_offline: bool,
    pub partitions: Vec<PartitionDto>,
}

/// Một phân vùng trên đĩa.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartitionDto {
    pub number: u32,
    pub offset: u64,
    pub size: u64,
    /// "Basic" | "Reserved" | "System" | "Recovery" | ...
    pub kind: String,
    pub letter: String,
    pub label: String,
    pub fs: String,
    pub free: u64,
    pub is_boot: bool,
    pub is_system: bool,
    pub is_active: bool,
    pub is_hidden: bool,
    pub is_read_only: bool,
    pub min_size: u64,
    pub max_size: u64,
    pub gpt_type: String,
    /// "On" nếu phân vùng đang được BitLocker bảo vệ.
    pub bitlocker: String,
}

/// Vùng trống (chưa phân bổ) trên đĩa.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub start: u64,
    pub size: u64,
}

impl Gap {
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.size)
    }
}

impl DiskDto {
    pub fn partition(&self, number: u32) -> Option<&PartitionDto> {
        self.partitions.iter().find(|p| p.number == number)
    }

    /// Toàn bộ vùng chưa phân bổ (kể cả phần đệm nhỏ ở đầu/cuối đĩa).
    pub fn all_gaps(&self) -> Vec<Gap> {
        let mut parts: Vec<&PartitionDto> = self.partitions.iter().filter(|p| p.size > 0).collect();
        parts.sort_by_key(|p| p.offset);
        let mut out = Vec::new();
        let mut cur = 0u64;
        for p in parts {
            if p.offset > cur {
                out.push(Gap {
                    start: cur,
                    size: p.offset - cur,
                });
            }
            cur = cur.max(p.offset.saturating_add(p.size));
        }
        if cur < self.size {
            out.push(Gap {
                start: cur,
                size: self.size - cur,
            });
        }
        out
    }

    /// Vùng trống có thể dùng để tạo phân vùng mới (đã căn 1 MiB, tối thiểu 16 MiB).
    pub fn usable_gaps(&self) -> Vec<Gap> {
        let mut out = Vec::new();
        for g in self.all_gaps() {
            let start = align_up(g.start.max(MIB), MIB);
            let end = align_down(g.end(), MIB);
            if end > start && end - start >= 16 * MIB {
                out.push(Gap {
                    start,
                    size: end - start,
                });
            }
        }
        out
    }

    /// Tổng dung lượng trống có thể dùng.
    pub fn free_bytes(&self) -> u64 {
        self.usable_gaps().iter().map(|g| g.size).sum()
    }

    pub fn role(&self) -> &'static str {
        if self.is_system || self.is_boot {
            "Hệ thống"
        } else {
            "Dữ liệu"
        }
    }

    pub fn protected(&self) -> bool {
        self.is_system || self.is_boot
    }

    pub fn healthy(&self) -> bool {
        self.health.eq_ignore_ascii_case("healthy") || self.health.is_empty()
    }
}

impl PartitionDto {
    pub fn is_protected(&self) -> bool {
        self.is_boot
            || self.is_system
            || self.kind.eq_ignore_ascii_case("reserved")
            || self.kind.eq_ignore_ascii_case("system")
    }

    pub fn letter_opt(&self) -> Option<char> {
        self.letter
            .chars()
            .next()
            .filter(|c| c.is_ascii_alphabetic())
    }

    pub fn display_name(&self) -> String {
        if !self.label.trim().is_empty() {
            self.label.clone()
        } else if let Some(c) = self.letter_opt() {
            format!("Ổ {c}:")
        } else {
            format!("Phân vùng #{}", self.number)
        }
    }

    pub fn used(&self) -> u64 {
        self.size.saturating_sub(self.free)
    }

    pub fn usage_ratio(&self) -> f32 {
        if self.size == 0 {
            0.0
        } else {
            (self.used() as f64 / self.size as f64) as f32
        }
    }

    /// Có thể thu nhỏ (shrink) an toàn bằng Windows Resize-Partition?
    pub fn can_shrink(&self) -> bool {
        !self.is_protected()
            && !self.bitlocker_on()
            && self.size > self.used().max(self.min_size)
            && matches!(self.fs.as_str(), "NTFS" | "ReFS")
    }

    pub fn can_extend(&self) -> bool {
        !self.is_protected()
            && !self.bitlocker_on()
            && self.max_size > self.size
            && !self.fs.is_empty()
    }

    pub fn bitlocker_on(&self) -> bool {
        self.bitlocker.eq_ignore_ascii_case("on")
    }

    pub fn can_format(&self) -> bool {
        !self.is_protected() && !self.kind.eq_ignore_ascii_case("reserved")
    }

    pub fn flags_text(&self) -> String {
        let mut f = Vec::new();
        if self.is_boot {
            f.push("Boot");
        }
        if self.is_system {
            f.push("Hệ thống");
        }
        if self.is_active {
            f.push("Active");
        }
        if self.is_hidden {
            f.push("Ẩn");
        }
        if self.is_read_only {
            f.push("Read-only");
        }
        if self.kind.eq_ignore_ascii_case("reserved") {
            f.push("Dự phòng");
        }
        if self.bitlocker_on() {
            f.push("BitLocker");
        }
        f.join(" · ")
    }
}

/// Căn lên bội số gần nhất.
pub fn align_up(n: u64, align: u64) -> u64 {
    if align == 0 {
        return n;
    }
    n.div_ceil(align) * align
}

/// Căn xuống bội số gần nhất.
pub fn align_down(n: u64, align: u64) -> u64 {
    if align == 0 {
        return n;
    }
    n / align * align
}

/// Định dạng dung lượng thân thiện: 512 GB, 1.8 TB, 500 MB.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut x = n as f64;
    let mut unit = 0usize;
    while x >= 1024.0 && unit + 1 < UNITS.len() {
        x /= 1024.0;
        unit += 1;
    }
    let s = if x >= 100.0 {
        format!("{x:.0}")
    } else if x >= 10.0 {
        format!("{x:.1}")
    } else {
        format!("{x:.2}")
    };
    let s = s.replace('.', ",");
    format!("{s} {}", UNITS[unit])
}

/// Định dạng dung lượng ngắn không khoảng trắng (dùng trong nhãn nhỏ).
pub fn short_bytes(n: u64) -> String {
    human_bytes(n).replace(' ', "")
}

/// Các ký tự ổ đĩa đang được dùng.
pub fn used_letters(disks: &[DiskDto]) -> Vec<char> {
    let mut v: Vec<char> = disks
        .iter()
        .flat_map(|d| d.partitions.iter())
        .filter_map(|p| p.letter_opt())
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Các ký tự ổ đĩa còn trống (bỏ A, B).
pub fn free_letters(disks: &[DiskDto]) -> Vec<char> {
    let used = used_letters(disks);
    ('C'..='Z').filter(|c| !used.contains(c)).collect()
}

/// Phân tích JSON do PowerShell trả về thành danh sách đĩa.
///
/// Chấp nhận cả trường hợp PowerShell "bung" mảng 1 phần tử thành object.
pub fn parse_disks(json: &str) -> Result<Vec<DiskDto>, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| format!("Dữ liệu đĩa không hợp lệ: {e}\n{}", truncate(json, 400)))?;
    let mut items = match v {
        Value::Object(mut map) => match map.remove("disks") {
            Some(Value::Array(a)) => a,
            Some(Value::Object(o)) => vec![Value::Object(o)],
            Some(Value::Null) | None => Vec::new(),
            Some(other) => vec![other],
        },
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        other => vec![other],
    };
    // Chuẩn hoá: partitions luôn là mảng.
    for item in items.iter_mut() {
        if let Value::Object(map) = item {
            match map.get_mut("partitions") {
                Some(Value::Object(_)) => {
                    let old = map.remove("partitions").unwrap();
                    map.insert("partitions".into(), Value::Array(vec![old]));
                }
                Some(Value::Null) | None => {
                    map.insert("partitions".into(), Value::Array(Vec::new()));
                }
                _ => {}
            }
        }
    }
    items
        .into_iter()
        .map(|x| {
            serde_json::from_value::<DiskDto>(x)
                .map_err(|e| format!("Thiếu trường dữ liệu đĩa: {e}"))
        })
        .collect()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk_with(parts: Vec<PartitionDto>) -> DiskDto {
        DiskDto {
            size: 100 * MIB,
            partitions: parts,
            ..Default::default()
        }
    }

    fn part(n: u32, offset: u64, size: u64) -> PartitionDto {
        PartitionDto {
            number: n,
            offset,
            size,
            ..Default::default()
        }
    }

    #[test]
    fn gaps_are_computed() {
        let d = disk_with(vec![
            part(1, 10 * MIB, 20 * MIB),
            part(2, 50 * MIB, 10 * MIB),
        ]);
        let g = d.all_gaps();
        assert_eq!(g.len(), 3);
        assert_eq!(
            g[0],
            Gap {
                start: 0,
                size: 10 * MIB
            }
        );
        assert_eq!(
            g[1],
            Gap {
                start: 30 * MIB,
                size: 20 * MIB
            }
        );
        assert_eq!(
            g[2],
            Gap {
                start: 60 * MIB,
                size: 40 * MIB
            }
        );
        let u = d.usable_gaps();
        // Vùng [0, 10 MB) sau khi căn chỉnh chỉ còn 9 MB nên bị loại.
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].start, 30 * MIB);
        assert_eq!(u[1].start, 60 * MIB);
    }

    #[test]
    fn parse_single_object_partitions() {
        let json = r#"{"disks":[{"number":0,"name":"X","size":1000,"partitions":{"number":1,"offset":1048576,"size":2097152}}]}"#;
        let d = parse_disks(json).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].partitions.len(), 1);
        assert_eq!(d[0].partitions[0].number, 1);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(GIB), "1,00 GB");
        assert_eq!(human_bytes(500 * 1024 * MIB), "500 GB");
    }
}
