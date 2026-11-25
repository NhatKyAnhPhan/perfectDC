//! Các thao tác quản lý ổ đĩa có kiểm tra an toàn.
//!
//! Ứng dụng dùng Windows Storage API (PowerShell ẩn) — chính là lớp mà
//! Disk Management của Windows sử dụng — nên việc thu nhỏ/mở rộng NTFS,
//! tạo/định dạng phân vùng được xử lý an toàn bởi Windows.

use crate::model::{self, DiskDto, MIB, align_down, free_letters, used_letters};
use crate::sys;

const MIN_PART_MIB: u64 = 16;
const MIN_KEEP_MIB: u64 = 512;
const FAT32_MAX: u64 = 32 * 1024 * MIB;

/// Hệ tệp đích khi định dạng.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fs {
    Ntfs,
    ExFat,
    Fat32,
}

impl Fs {
    pub fn as_str(self) -> &'static str {
        match self {
            Fs::Ntfs => "NTFS",
            Fs::ExFat => "exFAT",
            Fs::Fat32 => "FAT32",
        }
    }

    pub fn all() -> [Fs; 3] {
        [Fs::Ntfs, Fs::ExFat, Fs::Fat32]
    }

    pub fn validate_size(self, size: u64) -> Result<(), String> {
        if self == Fs::Fat32 && size > FAT32_MAX {
            return Err(
                "FAT32 chỉ hỗ trợ phân vùng tối đa 32 GB. Hãy chọn NTFS hoặc exFAT.".into(),
            );
        }
        Ok(())
    }
}

/// Một thao tác ghi lên đĩa. Mọi thao tác đều phải qua `validate` trước khi chạy.
#[derive(Clone, Debug)]
pub enum Action {
    Create {
        disk: u32,
        offset: u64,
        size: u64,
        fs: Fs,
        label: String,
        letter: Option<char>,
    },
    Split {
        disk: u32,
        partition: u32,
        new_size: u64,
        fs: Fs,
        label: String,
        letter: Option<char>,
    },
    Extend {
        disk: u32,
        partition: u32,
        size: u64,
    },
    Delete {
        disk: u32,
        partition: u32,
    },
    Format {
        disk: u32,
        partition: u32,
        fs: Fs,
        label: String,
        full: bool,
    },
    SetLetter {
        disk: u32,
        partition: u32,
        letter: char,
    },
    Initialize {
        disk: u32,
    },
    Wipe {
        disk: u32,
    },
}

impl Action {
    pub fn title(&self) -> &'static str {
        match self {
            Action::Create { .. } => "Tạo phân vùng mới",
            Action::Split { .. } => "Chia ổ đĩa",
            Action::Extend { .. } => "Mở rộng phân vùng",
            Action::Delete { .. } => "Xoá phân vùng",
            Action::Format { .. } => "Định dạng phân vùng",
            Action::SetLetter { .. } => "Đổi ký tự ổ đĩa",
            Action::Initialize { .. } => "Khởi tạo đĩa",
            Action::Wipe { .. } => "Xoá sạch toàn bộ đĩa",
        }
    }

    /// Từ khoá người dùng phải gõ để xác nhận (thao tác nguy hiểm).
    pub fn keyword(&self) -> Option<&'static str> {
        match self {
            Action::Delete { .. } => Some("DELETE"),
            Action::Format { .. } => Some("FORMAT"),
            Action::Wipe { .. } => Some("ERASE"),
            Action::Initialize { .. } => Some("INIT"),
            _ => None,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Action::Create {
                disk,
                offset,
                size,
                fs,
                letter,
                ..
            } => {
                format!(
                    "Tạo phân vùng {} trên đĩa {} tại offset {} (ký tự {})",
                    model::human_bytes(*size),
                    disk,
                    model::human_bytes(*offset),
                    letter
                        .map(|c| format!("{c}:"))
                        .unwrap_or_else(|| "không gán".into())
                ) + &format!(", định dạng {}", fs.as_str())
            }
            Action::Split {
                disk,
                partition,
                new_size,
                fs,
                letter,
                ..
            } => {
                format!(
                    "Chia phân vùng #{} (đĩa {}) — tạo ổ mới {} {}",
                    partition,
                    disk,
                    model::human_bytes(*new_size),
                    fs.as_str()
                ) + &letter.map(|c| format!(" ({c}:)")).unwrap_or_default()
            }
            Action::Extend {
                disk,
                partition,
                size,
            } => format!(
                "Mở rộng phân vùng #{} (đĩa {}) lên {}",
                partition,
                disk,
                model::human_bytes(*size)
            ),
            Action::Delete { disk, partition } => {
                format!("Xoá phân vùng #{} trên đĩa {}", partition, disk)
            }
            Action::Format {
                disk,
                partition,
                fs,
                full,
                ..
            } => format!(
                "Định dạng {} phân vùng #{} (đĩa {}){}",
                fs.as_str(),
                partition,
                disk,
                if *full {
                    " — định dạng đầy đủ"
                } else {
                    " — định dạng nhanh"
                }
            ),
            Action::SetLetter {
                disk,
                partition,
                letter,
            } => format!(
                "Gán ký tự {}: cho phân vùng #{} (đĩa {})",
                letter, partition, disk
            ),
            Action::Initialize { disk } => format!("Khởi tạo đĩa {} với bảng phân vùng GPT", disk),
            Action::Wipe { disk } => {
                format!("XOÁ SẠCH toàn bộ dữ liệu & phân vùng trên đĩa {}", disk)
            }
        }
    }

    /// Kiểm tra tính hợp lệ dựa trên trạng thái đĩa hiện tại.
    pub fn validate(&self, disks: &[DiskDto]) -> Result<(), String> {
        let get_disk = |n: u32| {
            disks
                .iter()
                .find(|d| d.number == n)
                .ok_or_else(|| format!("Không tìm thấy đĩa {n}."))
        };
        let get_part = |disk: &DiskDto, p: u32| {
            disk.partition(p)
                .cloned()
                .ok_or_else(|| format!("Không tìm thấy phân vùng #{p} trên đĩa {}.", disk.number))
        };

        match self {
            Action::Create {
                disk,
                offset,
                size,
                fs,
                letter,
                ..
            } => {
                let d = get_disk(*disk)?;
                ensure_disk_writable(d)?;
                if d.style == "RAW" {
                    return Err("Đĩa chưa được khởi tạo (RAW). Hãy khởi tạo đĩa trước.".into());
                }
                if *size < MIN_PART_MIB * MIB {
                    return Err(format!("Kích thước tối thiểu là {MIN_PART_MIB} MB."));
                }
                if *size % MIB != 0 {
                    return Err("Kích thước phải là bội số của 1 MB.".into());
                }
                if *offset % MIB != 0 || *offset < MIB {
                    return Err("Vị trí bắt đầu phải căn chỉnh 1 MB và ≥ 1 MB.".into());
                }
                if offset.saturating_add(*size) > d.size {
                    return Err("Phân vùng vượt quá dung lượng đĩa.".into());
                }
                for p in &d.partitions {
                    if *offset < p.offset.saturating_add(p.size)
                        && offset.saturating_add(*size) > p.offset
                    {
                        return Err(format!("Vùng chọn bị chiếm bởi phân vùng #{}.", p.number));
                    }
                }
                fs.validate_size(*size)?;
                if let Some(l) = letter {
                    check_letter(*l, disks)?;
                }
                Ok(())
            }
            Action::Split {
                disk,
                partition,
                new_size,
                fs,
                letter,
                ..
            } => {
                let d = get_disk(*disk)?;
                ensure_disk_writable(d)?;
                let p = get_part(d, *partition)?;
                ensure_part_editable(&p)?;
                if p.bitlocker_on() {
                    return Err(
                        "Phân vùng đang bật BitLocker. Hãy tắt BitLocker (hoặc suspend) trước khi chia ổ."
                            .into(),
                    );
                }
                if !matches!(p.fs.as_str(), "NTFS" | "ReFS") {
                    return Err(format!(
                        "Chỉ có thể thu nhỏ phân vùng NTFS/ReFS (phân vùng này là {}).",
                        if p.fs.is_empty() {
                            "chưa định dạng"
                        } else {
                            &p.fs
                        }
                    ));
                }
                if *new_size < 256 * MIB {
                    return Err("Phân vùng mới phải có tối thiểu 256 MB.".into());
                }
                if *new_size % MIB != 0 {
                    return Err("Kích thước phải là bội số của 1 MB.".into());
                }
                let keep = p.size.saturating_sub(*new_size);
                let min_keep = p.min_size.max(p.used()).max(MIN_KEEP_MIB * MIB);
                if keep < min_keep {
                    return Err(format!(
                        "Không đủ chỗ: cần giữ lại ít nhất {} cho phân vùng hiện tại (dữ liệu đang dùng {}).",
                        model::human_bytes(min_keep),
                        model::human_bytes(p.used())
                    ));
                }
                fs.validate_size(*new_size)?;
                if let Some(l) = letter {
                    check_letter(*l, disks)?;
                }
                Ok(())
            }
            Action::Extend {
                disk,
                partition,
                size,
            } => {
                let d = get_disk(*disk)?;
                ensure_disk_writable(d)?;
                let p = get_part(d, *partition)?;
                ensure_part_editable(&p)?;
                if p.bitlocker_on() {
                    return Err(
                        "Phân vùng đang bật BitLocker. Hãy tắt BitLocker (hoặc suspend) trước khi mở rộng."
                            .into(),
                    );
                }
                if !p.can_extend() {
                    return Err(
                        "Phân vùng này không thể mở rộng (không có vùng trống liền kề hoặc cần quyền Admin)."
                            .into(),
                    );
                }
                if *size <= p.size {
                    return Err("Kích thước mới phải lớn hơn kích thước hiện tại.".into());
                }
                if *size % MIB != 0 {
                    return Err("Kích thước phải là bội số của 1 MB.".into());
                }
                if p.max_size > 0 && *size > p.max_size {
                    return Err(format!(
                        "Tối đa có thể mở rộng lên {}.",
                        model::human_bytes(p.max_size)
                    ));
                }
                Ok(())
            }
            Action::Delete { disk, partition } => {
                let d = get_disk(*disk)?;
                ensure_disk_writable(d)?;
                let p = get_part(d, *partition)?;
                ensure_part_editable(&p)?;
                Ok(())
            }
            Action::Format {
                disk,
                partition,
                fs,
                label,
                ..
            } => {
                let d = get_disk(*disk)?;
                ensure_disk_writable(d)?;
                let p = get_part(d, *partition)?;
                ensure_part_editable(&p)?;
                if p.kind.eq_ignore_ascii_case("reserved") {
                    return Err("Không thể định dạng phân vùng dự phòng của Windows.".into());
                }
                if p.bitlocker_on() {
                    return Err(
                        "Phân vùng đang bật BitLocker. Hãy tắt BitLocker trước khi định dạng."
                            .into(),
                    );
                }
                fs.validate_size(p.size)?;
                if label.chars().count() > 32 {
                    return Err("Nhãn ổ đĩa tối đa 32 ký tự.".into());
                }
                Ok(())
            }
            Action::SetLetter {
                disk,
                partition,
                letter,
            } => {
                let d = get_disk(*disk)?;
                ensure_disk_writable(d)?;
                let p = get_part(d, *partition)?;
                if p.kind.eq_ignore_ascii_case("reserved") {
                    return Err("Không thể gán ký tự cho phân vùng dự phòng.".into());
                }
                if p.letter_opt() == Some(*letter) {
                    return Err("Phân vùng đã dùng ký tự này.".into());
                }
                check_letter(*letter, disks)?;
                Ok(())
            }
            Action::Initialize { disk } => {
                let d = get_disk(*disk)?;
                ensure_disk_writable(d)?;
                if d.style != "RAW" {
                    return Err(format!(
                        "Đĩa đã được khởi tạo ({}) — thao tác này chỉ dành cho đĩa RAW.",
                        d.style
                    ));
                }
                Ok(())
            }
            Action::Wipe { disk } => {
                let d = get_disk(*disk)?;
                if d.protected() || d.partitions.iter().any(|p| p.is_boot || p.is_system) {
                    return Err("KHÔNG THỂ xoá đĩa chứa hệ điều hành đang chạy.".into());
                }
                ensure_disk_writable(d)?;
                Ok(())
            }
        }
    }

    /// Thực thi thao tác (đã xác nhận). Trả về thông điệp thành công.
    pub fn run(&self) -> Result<String, String> {
        if !sys::is_admin() {
            return Err(
                "Cần quyền Administrator để thay đổi ổ đĩa. Nhấn “Chạy với quyền Admin”.".into(),
            );
        }
        let script = self.script();
        sys::append_log("ACTION", &self.description());
        sys::append_log("SCRIPT", &script);
        match sys::powershell(&script) {
            Ok(out) => {
                sys::append_log("OK", &self.description());
                let extra = out.trim();
                if extra.is_empty() {
                    Ok(format!("{} — hoàn tất.", self.description()))
                } else {
                    Ok(format!("{} — hoàn tất.\n{}", self.description(), extra))
                }
            }
            Err(e) => {
                sys::append_log("ERROR", &format!("{} → {}", self.description(), e));
                Err(e)
            }
        }
    }

    /// Sinh script PowerShell thực thi (có kiểm tra lại trạng thái ngay trước khi ghi).
    fn script(&self) -> String {
        match self {
            Action::Create {
                disk,
                offset,
                size,
                fs,
                label,
                letter,
            } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
$size = [uint64]{size}; $offset = [uint64]{offset}
if ($size -lt {min}) {{ throw 'Kích thước quá nhỏ.' }}
if (($size % 1048576) -ne 0) {{ throw 'Kích thước chưa căn chỉnh 1 MB.' }}
if (($offset % 1048576) -ne 0 -or $offset -lt 1048576) {{ throw 'Offset chưa căn chỉnh 1 MB.' }}
if (($offset + $size) -gt [uint64]$disk.Size) {{ throw 'Vượt quá dung lượng đĩa.' }}
foreach ($p in @(Get-Partition -DiskNumber {disk} -ErrorAction SilentlyContinue)) {{
  if ($offset -lt ([uint64]$p.Offset + [uint64]$p.Size) -and ($offset + $size) -gt [uint64]$p.Offset) {{
    throw "Vùng bị chiếm bởi phân vùng $($p.PartitionNumber)."
  }}
}}
$np = New-Partition -DiskNumber {disk} -Offset $offset -Size $size{letter_arg} -ErrorAction Stop
"#,
                    min = MIN_PART_MIB * MIB,
                    letter_arg = letter
                        .map(|c| format!(" -DriveLetter {c}"))
                        .unwrap_or_default(),
                ));
                s.push_str(&format!(
                    "Format-Volume -Partition $np -FileSystem {}{} -Confirm:$false -ErrorAction Stop | Out-Null\n",
                    sys::ps_quote(fs.as_str()),
                    label_arg(label)
                ));
                s.push_str("Write-Output ('OK|' + $np.PartitionNumber)\n");
                s
            }
            Action::Split {
                disk,
                partition,
                new_size,
                fs,
                label,
                letter,
            } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
$p = Get-Partition -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
if ($p.IsBoot -or $p.IsSystem) {{ throw 'Không thể chia phân vùng hệ thống đang khởi động.' }}
if ($p.Type -eq 'Reserved') {{ throw 'Không thể chia phân vùng dự phòng.' }}
$oldSize = [uint64]$p.Size
$newSize = [uint64]{new_size}
$keep = $oldSize - $newSize
if ($newSize -lt 268435456) {{ throw 'Phân vùng mới quá nhỏ.' }}
if (($newSize % 1048576) -ne 0) {{ throw 'Kích thước chưa căn chỉnh 1 MB.' }}
if ($keep -lt 536870912) {{ throw 'Phân vùng hiện tại phải giữ lại ít nhất 512 MB.' }}
$sup = Get-PartitionSupportedSize -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
if ($keep -lt [uint64]$sup.SizeMin) {{
  throw ('Không thể thu nhỏ: cần giữ lại ít nhất ' + [math]::Round([uint64]$sup.SizeMin/1GB,2) + ' GB cho phân vùng hiện tại.')
}}
Resize-Partition -DiskNumber {disk} -PartitionNumber {partition} -Size $keep -Confirm:$false -ErrorAction Stop | Out-Null
try {{
  $p2 = Get-Partition -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
  $off = [uint64]$p2.Offset + [uint64]$p2.Size
  # Windows yêu cầu offset phân vùng mới căn chỉnh 1 MB.
  $off = [uint64]([math]::Ceiling($off / 1048576.0)) * 1048576
  $end = [uint64]$p.Offset + [uint64]$oldSize
  $space = $end - $off
  if ($space -lt $newSize) {{
    $newSize = [uint64]([math]::Floor($space / 1048576.0)) * 1048576
  }}
  if ($newSize -lt 268435456) {{ throw 'Không đủ chỗ liền kề để tạo phân vùng mới sau khi thu nhỏ.' }}
  $np = New-Partition -DiskNumber {disk} -Offset $off -Size $newSize{letter_arg} -ErrorAction Stop
  Format-Volume -Partition $np -FileSystem {fs}{label_arg} -Confirm:$false -ErrorAction Stop | Out-Null
}} catch {{
  if ($np) {{ try {{ Remove-Partition -DiskNumber {disk} -PartitionNumber $np.PartitionNumber -Confirm:$false -ErrorAction Stop | Out-Null }} catch {{ }} }}
  try {{ Resize-Partition -DiskNumber {disk} -PartitionNumber {partition} -Size $oldSize -Confirm:$false -ErrorAction Stop | Out-Null }} catch {{ }}
  throw
}}
Write-Output ('OK|' + $np.PartitionNumber)
"#,
                    letter_arg = letter
                        .map(|c| format!(" -DriveLetter {c}"))
                        .unwrap_or_default(),
                    fs = sys::ps_quote(fs.as_str()),
                    label_arg = label_arg(label),
                ));
                s
            }
            Action::Extend {
                disk,
                partition,
                size,
            } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
$p = Get-Partition -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
if ($p.IsBoot -or $p.IsSystem) {{ throw 'Không thể mở rộng phân vùng hệ thống đang khởi động.' }}
if ($p.Type -eq 'Reserved') {{ throw 'Không thể mở rộng phân vùng dự phòng.' }}
$newSize = [uint64]{size}
if ($newSize -le [uint64]$p.Size) {{ throw 'Kích thước mới phải lớn hơn hiện tại.' }}
$sup = Get-PartitionSupportedSize -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
if ($newSize -gt [uint64]$sup.SizeMax) {{
  throw ('Vượt quá mức tối đa: ' + [math]::Round([uint64]$sup.SizeMax/1GB,2) + ' GB.')
}}
Resize-Partition -DiskNumber {disk} -PartitionNumber {partition} -Size $newSize -Confirm:$false -ErrorAction Stop | Out-Null
Write-Output 'OK'
"#
                ));
                s
            }
            Action::Delete { disk, partition } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
$p = Get-Partition -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
if ($p.IsBoot -or $p.IsSystem) {{ throw 'Không thể xoá phân vùng hệ thống đang khởi động.' }}
if ($p.Type -eq 'Reserved') {{ throw 'Không thể xoá phân vùng dự phòng.' }}
Remove-Partition -DiskNumber {disk} -PartitionNumber {partition} -Confirm:$false -ErrorAction Stop
Write-Output 'OK'
"#
                ));
                s
            }
            Action::Format {
                disk,
                partition,
                fs,
                label,
                full,
            } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
$p = Get-Partition -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
if ($p.IsBoot -or $p.IsSystem) {{ throw 'Không thể định dạng phân vùng hệ thống đang khởi động.' }}
if ($p.Type -eq 'Reserved') {{ throw 'Không thể định dạng phân vùng dự phòng.' }}
Format-Volume -Partition $p -FileSystem {fs}{label_arg}{full} -Confirm:$false -ErrorAction Stop | Out-Null
Write-Output 'OK'
"#,
                    fs = sys::ps_quote(fs.as_str()),
                    label_arg = label_arg(label),
                    full = if *full { " -Full" } else { "" },
                ));
                s
            }
            Action::SetLetter {
                disk,
                partition,
                letter,
            } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
$p = Get-Partition -DiskNumber {disk} -PartitionNumber {partition} -ErrorAction Stop
if ($p.Type -eq 'Reserved') {{ throw 'Không thể gán ký tự cho phân vùng dự phòng.' }}
$used = @(Get-Volume | Where-Object {{ $_.DriveLetter }} | ForEach-Object {{ [string]$_.DriveLetter }})
if ($used -contains '{letter}') {{ throw 'Ký tự {letter}: đã được sử dụng.' }}
Set-Partition -DiskNumber {disk} -PartitionNumber {partition} -NewDriveLetter {letter} -ErrorAction Stop | Out-Null
Write-Output 'OK'
"#
                ));
                s
            }
            Action::Initialize { disk } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
if ($disk.PartitionStyle -ne 'RAW') {{ throw ('Đĩa đã được khởi tạo (' + $disk.PartitionStyle + ').') }}
if ($disk.IsSystem -or $disk.IsBoot) {{ throw 'Không thể khởi tạo đĩa hệ thống.' }}
Initialize-Disk -Number {disk} -PartitionStyle GPT -Confirm:$false
Write-Output 'OK'
"#
                ));
                s
            }
            Action::Wipe { disk } => {
                let mut s = common_header(*disk);
                s.push_str(&format!(
                    r#"
if ($disk.IsSystem -or $disk.IsBoot) {{ throw 'KHÔNG THỂ xoá đĩa chứa hệ điều hành.' }}
foreach ($p in @(Get-Partition -DiskNumber {disk} -ErrorAction SilentlyContinue)) {{
  if ($p.IsBoot -or $p.IsSystem) {{ throw 'KHÔNG THỂ xoá đĩa có phân vùng hệ thống/khởi động.' }}
}}
Clear-Disk -Number {disk} -RemoveData -RemoveOEM -Confirm:$false -ErrorAction Stop
Write-Output 'OK'
"#
                ));
                s
            }
        }
    }
}

fn label_arg(label: &str) -> String {
    if label.trim().is_empty() {
        String::new()
    } else {
        format!(" -NewFileSystemLabel {}", sys::ps_quote(label))
    }
}

fn common_header(disk: u32) -> String {
    format!(
        r#"$disk = Get-Disk -Number {disk} -ErrorAction Stop
if ($disk.IsReadOnly) {{ throw 'Đĩa đang ở chế độ chỉ đọc.' }}
if ($disk.IsOffline) {{ throw 'Đĩa đang offline.' }}
"#
    )
}

fn ensure_disk_writable(d: &DiskDto) -> Result<(), String> {
    if d.is_read_only {
        return Err(format!("Đĩa {} đang ở chế độ chỉ đọc.", d.number));
    }
    if d.is_offline {
        return Err(format!("Đĩa {} đang offline.", d.number));
    }
    Ok(())
}

fn ensure_part_editable(p: &model::PartitionDto) -> Result<(), String> {
    if p.is_boot || p.is_system {
        return Err(
            "Đây là phân vùng hệ thống/khởi động — không thể thay đổi để bảo vệ Windows.".into(),
        );
    }
    if p.kind.eq_ignore_ascii_case("reserved") {
        return Err("Đây là phân vùng dự phòng của Windows — không thể thay đổi.".into());
    }
    Ok(())
}

fn check_letter(letter: char, disks: &[DiskDto]) -> Result<(), String> {
    if !letter.is_ascii_alphabetic() {
        return Err("Ký tự ổ đĩa không hợp lệ.".into());
    }
    let l = letter.to_ascii_uppercase();
    if l < 'C' {
        return Err("Chỉ có thể dùng ký tự ổ đĩa từ C: đến Z:.".into());
    }
    if used_letters(disks).contains(&l) {
        return Err(format!("Ký tự {l}: đã được sử dụng."));
    }
    Ok(())
}

/// Gợi ý ký tự ổ đĩa còn trống (ưu tiên D, E, F...).
pub fn suggest_letter(disks: &[DiskDto]) -> Option<char> {
    let free = free_letters(disks);
    ['D', 'E', 'F', 'G', 'H']
        .into_iter()
        .find(|c| free.contains(c))
        .or_else(|| free.first().copied())
}

/// Kích thước gợi ý khi tạo phân vùng mới: toàn bộ vùng trống (đã căn chỉnh).
pub fn suggested_create_size(gap_size: u64) -> u64 {
    align_down(gap_size, MIB).max(MIN_PART_MIB * MIB)
}

/// Kích thước nhỏ nhất hợp lệ cho phân vùng mới khi chia ổ.
pub fn min_split_size() -> u64 {
    256 * MIB
}

/// Kích thước tối đa có thể tách ra khỏi một phân vùng.
pub fn max_split_size(p: &model::PartitionDto) -> u64 {
    let min_keep = p.min_size.max(p.used()).max(MIN_KEEP_MIB * MIB);
    align_down(p.size.saturating_sub(min_keep), MIB)
}

/// Đọc danh sách đĩa qua Windows Storage API.
pub fn fetch_disks() -> Result<Vec<DiskDto>, String> {
    let script = FETCH_SCRIPT;
    let out = sys::powershell(script)?;
    if out.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut disks = model::parse_disks(&out)?;
    disks.sort_by_key(|d| d.number);
    Ok(disks)
}

const FETCH_SCRIPT: &str = r#"
$blMap = @{}
try {
  foreach ($bv in @(Get-BitLockerVolume -ErrorAction Stop)) {
    if ($bv.ProtectionStatus -eq 'On') { $blMap[[string]$bv.MountPoint] = $true }
  }
} catch { }
$result = foreach ($d in Get-Disk | Sort-Object Number) {
  $parts = @()
  foreach ($p in @(Get-Partition -DiskNumber $d.Number -ErrorAction SilentlyContinue)) {
    $v = $null
    try { $v = Get-Volume -Partition $p -ErrorAction Stop } catch { }
    $min = 0; $max = 0
    try {
      $s = Get-PartitionSupportedSize -DiskNumber $d.Number -PartitionNumber $p.PartitionNumber -ErrorAction Stop
      $min = [uint64]$s.SizeMin; $max = [uint64]$s.SizeMax
    } catch { }
    $letter = ''
    if ($p.DriveLetter) { $letter = [string]$p.DriveLetter }
    $parts += [pscustomobject]@{
      number=[int]$p.PartitionNumber; offset=[uint64]$p.Offset; size=[uint64]$p.Size; kind=[string]$p.Type
      letter=$letter; label=if ($v) {[string]$v.FileSystemLabel} else {''}; fs=if ($v) {[string]$v.FileSystem} else {''}
      free=if ($v) {[uint64]$v.SizeRemaining} else {[uint64]0}
      isBoot=[bool]$p.IsBoot; isSystem=[bool]$p.IsSystem; isActive=[bool]$p.IsActive; isHidden=[bool]$p.IsHidden
      isReadOnly=[bool]$p.IsReadOnly; minSize=[uint64]$min; maxSize=[uint64]$max; gptType=[string]$p.GptType
      bitlocker=if ($letter -and $blMap.ContainsKey(($letter + ':'))) {'On'} else {''}
    }
  }
  [pscustomobject]@{
    number=[int]$d.Number; name=[string]$d.FriendlyName; serial=([string]$d.SerialNumber).Trim()
    size=[uint64]$d.Size; style=[string]$d.PartitionStyle; busType=[string]$d.BusType
    health=[string]$d.HealthStatus; operational=(@($d.OperationalStatus) -join ', ')
    isBoot=[bool]$d.IsBoot; isSystem=[bool]$d.IsSystem; isReadOnly=[bool]$d.IsReadOnly; isOffline=[bool]$d.IsOffline
    partitions=@($parts)
  }
}
[pscustomobject]@{disks=@($result)} | ConvertTo-Json -Depth 6 -Compress
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(style: &str) -> DiskDto {
        DiskDto {
            number: 1,
            size: 1024 * MIB,
            style: style.into(),
            ..Default::default()
        }
    }

    #[test]
    fn create_validation() {
        let d = disk("GPT");
        let act = Action::Create {
            disk: 1,
            offset: MIB,
            size: 100 * MIB,
            fs: Fs::Ntfs,
            label: "x".into(),
            letter: Some('D'),
        };
        assert!(act.validate(std::slice::from_ref(&d)).is_ok());

        let bad = Action::Create {
            disk: 1,
            offset: MIB + 1,
            size: 100 * MIB,
            fs: Fs::Ntfs,
            label: String::new(),
            letter: None,
        };
        assert!(bad.validate(std::slice::from_ref(&d)).is_err());

        let raw = Action::Create {
            disk: 1,
            offset: MIB,
            size: 100 * MIB,
            fs: Fs::Ntfs,
            label: String::new(),
            letter: None,
        };
        assert!(raw.validate(&[disk("RAW")]).is_err());
    }

    #[test]
    fn fat32_limit() {
        assert!(Fs::Fat32.validate_size(40 * 1024 * MIB).is_err());
        assert!(Fs::Fat32.validate_size(30 * 1024 * MIB).is_ok());
        assert!(Fs::Ntfs.validate_size(40_000 * 1024 * MIB).is_ok());
    }

    #[test]
    fn wipe_system_disk_forbidden() {
        let mut d = disk("GPT");
        d.is_system = true;
        assert!(Action::Wipe { disk: 1 }.validate(&[d]).is_err());
    }

    #[test]
    fn split_size_helpers() {
        let mut p = model::PartitionDto {
            number: 1,
            offset: MIB,
            size: 100 * 1024 * MIB,
            free: 90 * 1024 * MIB,
            fs: "NTFS".into(),
            ..Default::default()
        };
        let max = max_split_size(&p);
        assert_eq!(max, 90 * 1024 * MIB);
        p.min_size = 20 * 1024 * MIB;
        assert_eq!(max_split_size(&p), 80 * 1024 * MIB);
        p.free = 0;
        assert_eq!(max_split_size(&p), 0);
    }

    #[test]
    fn letters_are_unique() {
        let mut a = disk("GPT");
        a.partitions.push(model::PartitionDto {
            letter: "C".into(),
            ..Default::default()
        });
        assert!(check_letter('C', &[a.clone()]).is_err());
        assert!(check_letter('D', &[a]).is_ok());
    }

    #[test]
    fn scripts_quote_labels() {
        let act = Action::Format {
            disk: 3,
            partition: 2,
            fs: Fs::Ntfs,
            label: "O'Reilly".into(),
            full: false,
        };
        let s = act.script();
        assert!(s.contains("'O''Reilly'"));
        assert!(s.contains("Get-Partition -DiskNumber 3 -PartitionNumber 2"));
    }
}
