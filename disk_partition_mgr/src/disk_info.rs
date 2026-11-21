//! Module đọc thông tin đĩa: kích thước đĩa, sector size, danh sách phân vùng GPT/MBR.
//!
//! Các Win32/IOCTL API được dùng tới (để người dùng có thể kiểm thử thực tế trên Windows):
//!
//! - `CreateFileW` (`Win32::Storage::FileSystem`) — mở handle tới đĩa vật lý
//!   (`\\.\PhysicalDriveN`) hoặc file ảnh (.vhd).
//! - `DeviceIoControl` (`Win32::System::IO`) — hai IOCTL:
//!   - `IOCTL_DISK_GET_DRIVE_LAYOUT_EX`: giải mã bảng phân vùng qua
//!     `DRIVE_LAYOUT_INFORMATION_EX` / `PARTITION_INFORMATION_EX`.
//!   - `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`: kích thước đĩa + sector size qua `DISK_GEOMETRY_EX`.
//! - `ReadFile`/`WriteFile`/`SetFilePointerEx`/`GetFileSizeEx` (`Win32::Storage::FileSystem`) —
//!   đọc/ghi byte tại offset tuyện đối (dùng để đọc header phân vùng và copy dữ liệu khi `--apply`).
//! - Phát hiện BitLocker qua chữ ký FVE volume header `-FVE-FS-` và identifier GUID
//!   (theo định dạng FVE volume header của libbde — libyal).
//! - Khi đĩa chỉ là file ảnh (VHD chưa mount): phân tích bảng phân vùng từ byte file
//!   (MBR tại LBA 0 / GPT tại LBA 1), sector size mặc định 512 B.

use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, GENERIC_READ, GENERIC_WRITE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileSizeEx, ReadFile, SetFilePointerEx, WriteFile,
    FILE_BEGIN, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    DRIVE_LAYOUT_INFORMATION_EX, DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
    IOCTL_DISK_GET_DRIVE_LAYOUT_EX, PARTITION_INFORMATION_EX, PARTITION_STYLE_GPT,
    PARTITION_STYLE_RAW,
};

/// Loại bảng phân vùng của đĩa.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// Bảng phân vùng MBR (PARTITION_STYLE_MBR).
    Mbr,
    /// Bảng phân vùng GPT (PARTITION_STYLE_GPT).
    Gpt,
    /// Bảng phân vùng không nhận diện (PARTITION_STYLE_RAW) — đĩa ẩn/đĩa chỉ có header.
    Raw,
    /// File ảnh (VHD/...) — chưa giải mã bảng phân vùng qua IOCTL.
    File,
}

impl Style {
    pub fn as_str(&self) -> &'static str {
        match self {
            Style::Mbr => "MBR",
            Style::Gpt => "GPT",
            Style::Raw => "RAW",
            Style::File => "FILE",
        }
    }
}

/// Thông tin một phân vùng.
#[derive(Clone, Debug)]
pub struct PartitionInfo {
    /// Số thứ tự phân vùng (1-based; 0 nếu RAW).
    pub number: u32,
    pub style: Style,
    /// Offset bắt đầu của phân vùng (bytes, absolute).
    pub start_offset: u64,
    /// Độ dài phân vùng (bytes).
    pub length: u64,
    /// MBR only: PartitionType (byte).
    pub mbr_type: u8,
    /// MBR only: BootIndicator (status byte != 0 → bootable).
    pub boot_indicator: bool,
    /// MBR only: RecognizedPartition.
    pub recognized_partition: bool,
    /// GPT only: PartitionType GUID (định dạng on-disk byte order).
    pub gpt_type_guid: Option<[u8; 16]>,
    /// GPT only: Attributes (ULONG64).
    pub gpt_attributes: u64,
    /// GPT only: tên phân vùng (UTF-16LE, dừng tại byte 0).
    pub gpt_name: String,
    /// Cảnh báo: phân vùng chứa hệ thống (ESP / boot volume).
    pub is_system: bool,
    /// Cảnh báo: phân vùng đang bật BitLocker (đã mã hóa).
    pub is_encrypted: bool,
}

impl PartitionInfo {
    /// Chuỗi mô tả (dùng hiển thị trong `inspect`).
    pub fn describe(&self) -> String {
        let mut s = match self.style {
            Style::Mbr => mbr_type_label(self.mbr_type),
            Style::Gpt => {
                let g = self.gpt_type_guid.unwrap_or([0u8; 16]);
                if self.gpt_name.is_empty() {
                    format!("GUID {}", guid_to_string(&g))
                } else {
                    format!("GUID {} \"{}\"", guid_to_string(&g), self.gpt_name)
                }
            }
            Style::Raw => "RAW".to_string(),
            Style::File => "FILE".to_string(),
        };
        if self.style == Style::Gpt && self.gpt_attributes != 0 {
            s.push_str(&format!(" [Attrs 0x{:016X}]", self.gpt_attributes));
        }
        let flags = self.flags();
        if !flags.is_empty() {
            s.push(' ');
            s.push_str(&flags);
        }
        s
    }

    /// Các cờ cảnh báo (Bootable, System, BitLocker, NotRecognized, Attrs) dạng `[A, B]` hoặc chuỗi rỗng.
    pub fn flags(&self) -> String {
        let mut f: Vec<String> = Vec::new();
        if self.boot_indicator {
            f.push("Bootable".to_string());
        }
        if self.is_system {
            f.push("System".to_string());
        }
        if self.is_encrypted {
            f.push("BitLocker".to_string());
        }
        if self.style == Style::Mbr && !self.recognized_partition {
            f.push("NotRecognized".to_string());
        }
        if f.is_empty() {
            String::new()
        } else {
            format!("[{}]", f.join(", "))
        }
    }
}

/// Thông tin đĩa đọc được.
#[derive(Clone, Debug)]
pub struct DiskInfo {
    pub path: String,
    pub size_bytes: u64,
    pub sector_size: u32,
    pub style: Style,
    pub partitions: Vec<PartitionInfo>,
}

impl DiskInfo {
    /// Phân vùng theo số (PartitionNumber).
    pub fn partition_by_number(&self, number: u32) -> Option<&PartitionInfo> {
        self.partitions.iter().find(|p| p.number == number)
    }

    /// Các vùng chưa phân bổ (unallocated): danh sách `(start, end_exclusive)`.
    pub fn gaps(&self) -> Vec<(u64, u64)> {
        let mut parts = self.partitions.clone();
        parts.sort_by_key(|p| p.start_offset);
        let mut out = Vec::new();
        let mut cur = 0u64;
        for p in parts {
            if p.start_offset > cur {
                out.push((cur, p.start_offset));
            }
            cur = cur.max(p.start_offset.saturating_add(p.length));
        }
        if cur < self.size_bytes {
            out.push((cur, self.size_bytes));
        }
        out
    }
}

/// Chế độ mở handle đ�.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Read,
    ReadWrite,
}

/// Nếu đường dẫn là `PhysicalDriveN` (tùy chọn có tiền tố `\\.\` hoặc `\\?\`),
/// trả về số N; trả về `None` cho các đường dẫn file ảnh khác.
pub fn physical_drive_index(path: &str) -> Option<u32> {
    let lower = path.to_ascii_lowercase();
    let idx = lower.find("physicaldrive")?;
    lower[idx + "physicaldrive".len()..]
        .trim_start_matches('\\')
        .parse::<u32>()
        .ok()
}

/// Đường dẫn chuẩn cho đĩa vật lý: `\\.\PhysicalDriveN`.
pub fn physical_drive_path(i: u32) -> String {
    let mut s = String::new();
    s.push('\\');
    s.push('\\');
    s.push('.');
    s.push('\\');
    s.push_str("PhysicalDrive");
    s.push_str(&i.to_string());
    s
}

pub fn is_physical_drive_path(path: &str) -> bool {
    physical_drive_index(path).is_some()
}

fn normalize_path(path: &str) -> String {
    match physical_drive_index(path) {
        Some(i) => physical_drive_path(i),
        None => path.to_string(),
    }
}

fn to_pcwstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_disk_inner(path: &str, mode: Mode) -> Result<HANDLE, String> {
    let desired = match mode {
        Mode::Read => GENERIC_READ,
        Mode::ReadWrite => GENERIC_READ | GENERIC_WRITE,
    };
    let name = to_pcwstr(path);
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            desired,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(format!("CreateFileW({path:?}) thất bại: {}", unsafe { GetLastError() }));
    }
    Ok(h)
}

pub fn open_disk(path: &str, mode: Mode) -> Result<HANDLE, String> {
    open_disk_inner(&normalize_path(path), mode)
}

pub fn close_disk(h: HANDLE) {
    unsafe {
        CloseHandle(h);
    }
}

/// Đọc thông tin đĩa (IOCTL cho đĩa vật lý; phân tích byte cho file ảnh).
pub fn read_disk(path: &str, mode: Mode) -> Result<DiskInfo, String> {
    let path = normalize_path(path);
    let h = open_disk_inner(&path, mode)?;
    let info = if is_physical_drive_path(&path) {
        ioctl_disk_info(h)
    } else {
        file_disk_info(h)
    };
    close_disk(h);
    info.map(|mut d| {
        d.path = path;
        d
    })
}

/// Liệt kê các đĩa vật lý đang có (từ 0).
pub fn list_physical_drives() -> Vec<u32> {
    let mut out = Vec::new();
    for i in 0..64u32 {
        match open_disk_inner(&physical_drive_path(i), Mode::Read) {
            Ok(h) => {
                close_disk(h);
                out.push(i);
            }
            Err(_) => break,
        }
    }
    out
}

/// Đọc bytes tại offset (SetFilePointerEx + ReadFile, vòng lặp đến khi đủ).
pub fn read_at(h: HANDLE, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    if buf.is_empty() {
        return Ok(0);
    }
    if unsafe { SetFilePointerEx(h, offset as i64, null_mut(), FILE_BEGIN) } == 0 {
        return Err(format!(
            "SetFilePointerEx (offset 0x{offset:X}) thất bại: {}",
            unsafe { GetLastError() }
        ));
    }
    let mut total = 0usize;
    while total < buf.len() {
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                h,
                buf[total..].as_mut_ptr(),
                (buf.len() - total) as u32,
                &mut read,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "ReadFile (offset 0x{:X}) thất bại: {}",
                offset + total as u64,
                unsafe { GetLastError() }
            ));
        }
        if read == 0 {
            break;
        }
        total += read as usize;
    }
    Ok(total)
}

/// Ghi bytes tại offset (SetFilePointerEx + WriteFile, vòng lặp đến khi đủ).
pub fn write_at(h: HANDLE, offset: u64, buf: &[u8]) -> Result<usize, String> {
    if buf.is_empty() {
        return Ok(0);
    }
    if unsafe { SetFilePointerEx(h, offset as i64, null_mut(), FILE_BEGIN) } == 0 {
        return Err(format!(
            "SetFilePointerEx (offset 0x{offset:X}) thất bại: {}",
            unsafe { GetLastError() }
        ));
    }
    let mut total = 0usize;
    while total < buf.len() {
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                h,
                buf[total..].as_ptr(),
                (buf.len() - total) as u32,
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "WriteFile (offset 0x{:X}) thất bại: {}",
                offset + total as u64,
                unsafe { GetLastError() }
            ));
        }
        if written == 0 {
            break;
        }
        total += written as usize;
    }
    Ok(total)
}

/// Copy len bytes từ `src` sang `dst` trên handle đã mở (Read+Write).
pub fn copy_range_h(h: HANDLE, src: u64, dst: u64, len: u64) -> Result<u64, String> {
    const CHUNK: u64 = 64 * 1024;
    let mut off = 0u64;
    let mut total = 0u64;
    while off < len {
        let n = (len - off).min(CHUNK) as usize;
        let mut buf = vec![0u8; n];
        let r = read_at(h, src + off, &mut buf)?;
        if r == 0 {
            return Err(format!("Không đọc được dữ liệu tại offset 0x{:X}", src + off));
        }
        write_at(h, dst + off, &buf[..r])?;
        off += r as u64;
        total += r as u64;
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// IOCTL helpers (đĩa vật lý)
// ---------------------------------------------------------------------------

fn device_ioctl(h: HANDLE, code: u32, buf: *mut c_void, size: u32) -> Result<u32, String> {
    let mut read: u32 = 0;
    let ok = unsafe { DeviceIoControl(h, code, null(), 0, buf, size, &mut read, null_mut()) };
    if ok == 0 {
        Err(format!(
            "DeviceIoControl (IOCTL 0x{code:X}) thất bại: {}",
            unsafe { GetLastError() }
        ))
    } else {
        Ok(read)
    }
}

/// IOCTL_DISK_GET_DRIVE_GEOMETRY_EX → (sector_size, disk_size).
fn ioctl_geometry(h: HANDLE) -> Result<(u64, u64), String> {
    let mut buf: Vec<u64> = vec![0; size_of::<DISK_GEOMETRY_EX>() / 8 + 1];
    device_ioctl(
        h,
        IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
        buf.as_mut_ptr().cast::<c_void>(),
        (buf.len() * 8) as u32,
    )?;
    let g = unsafe { &*(buf.as_ptr().cast::<DISK_GEOMETRY_EX>()) };
    Ok((g.Geometry.BytesPerSector as u64, g.DiskSize as u64))
}

fn ioctl_disk_info(h: HANDLE) -> Result<DiskInfo, String> {
    let (sector, size) = ioctl_geometry(h)?;
    let (style, mut partitions) = ioctl_layout(h)?;
    // Phát hiện BitLocker: đọc 512 bytes đầu của mỗi phân vùng.
    for p in partitions.iter_mut() {
        let mut head = [0u8; 512];
        let n = read_at(h, p.start_offset, &mut head).unwrap_or(0);
        if n > 0 {
            p.is_encrypted = contains_bde_header(&head[..n]);
        }
    }
    let sector = if sector == 0 { 512 } else { sector as u32 };
    Ok(DiskInfo {
        path: String::new(),
        size_bytes: size,
        sector_size: sector,
        style,
        partitions,
    })
}

fn style_of_style_i(s: i32) -> Style {
    match s {
        s if s == PARTITION_STYLE_GPT => Style::Gpt,
        s if s == PARTITION_STYLE_RAW => Style::Raw,
        _ => Style::Mbr,
    }
}

/// IOCTL_DISK_GET_DRIVE_LAYOUT_EX → (Style, Vec<PartitionInfo>).
fn ioctl_layout(h: HANDLE) -> Result<(Style, Vec<PartitionInfo>), String> {
    let entry_size = size_of::<PARTITION_INFORMATION_EX>();
    let layout_size = size_of::<DRIVE_LAYOUT_INFORMATION_EX>();
    // Bộ đệm đủ lớn cho tối đa 255 phân vùng (MBR: 4, GPT: tối đa MaxPartitionCount).
    let mut buf: Vec<u64> = vec![0; (layout_size + 255 * entry_size + 7) / 8 + 1];
    device_ioctl(
        h,
        IOCTL_DISK_GET_DRIVE_LAYOUT_EX,
        buf.as_mut_ptr().cast::<c_void>(),
        (buf.len() * 8) as u32,
    )?;
    let base = buf.as_ptr().cast::<u8>();
    let layout = unsafe { &*base.cast::<DRIVE_LAYOUT_INFORMATION_EX>() };
    let style = match layout.PartitionStyle as i32 {
        s if s == PARTITION_STYLE_GPT => Style::Gpt,
        s if s == PARTITION_STYLE_RAW => Style::Raw,
        _ => Style::Mbr,
    };
    let count = layout.PartitionCount as usize;
    if count > 255 {
        return Err(format!("Số lượng phân vùng bất thường: {count}"));
    }
    let entry_off = offset_of!(DRIVE_LAYOUT_INFORMATION_EX, PartitionEntry);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = unsafe { &*(base.add(entry_off + i * entry_size).cast::<PARTITION_INFORMATION_EX>()) };
        let style_i = e.PartitionStyle;
        let e_style = style_of_style_i(style_i);
        let (mbr_type, boot, recognized, gpt_type, gpt_attrs, gpt_name) = match style_i {
            s if s == PARTITION_STYLE_GPT => {
                let gpt = unsafe { e.Anonymous.Gpt };
                (
                    0u8,
                    false,
                    false,
                    Some(guid_bytes(&gpt.PartitionType)),
                    gpt.Attributes as u64,
                    gpt_name_to_string(&gpt.Name),
                )
            }
            s if s == PARTITION_STYLE_RAW => (0u8, false, false, None, 0u64, String::new()),
            _ => {
                let mbr = unsafe { e.Anonymous.Mbr };
                (
                    mbr.PartitionType,
                    mbr.BootIndicator != 0,
                    mbr.RecognizedPartition != 0,
                    None,
                    0u64,
                    String::new(),
                )
            }
        };
        let is_system = match style_i {
            s if s == PARTITION_STYLE_GPT => gpt_type == Some(ESP_GUID_BYTES),
            s if s == PARTITION_STYLE_RAW => false,
            _ => boot || mbr_type == 0xEF,
        };
        out.push(PartitionInfo {
            number: e.PartitionNumber,
            style: e_style,
            start_offset: e.StartingOffset as u64,
            length: e.PartitionLength as u64,
            mbr_type,
            boot_indicator: boot,
            recognized_partition: recognized,
            gpt_type_guid: gpt_type,
            gpt_attributes: gpt_attrs,
            gpt_name,
            is_system,
            is_encrypted: false,
        });
    }
    Ok((style, out))
}

// ---------------------------------------------------------------------------
// Phân tích byte cho file ảnh (.vhd chưa mount)
// ---------------------------------------------------------------------------

/// Chữ ký BitLocker `-FVE-FS-` tại offset 3 của FVE volume header (Vista format).
/// (theo libbde — libyal)
const BDE_SIGNATURE: &[u8] = b"-FVE-FS-";
/// Identifier GUID tại offset 0 (Windows 7/Vista/to-go format).
const BDE_IDENTIFIER: [u8; 16] = [
    0x3b, 0xd6, 0x67, 0x49, 0x29, 0x2e, 0xd8, 0x4a, 0x83, 0x99, 0xf6, 0xa3, 0x39, 0xe3, 0xd0, 0x01,
];
/// Identifier GUID tại offset 0 (Windows 7+ "used disk space only" format).
const BDE_IDENTIFIER_USED_DISK_SPACE_ONLY: [u8; 16] = [
    0x3b, 0x4d, 0xa8, 0x92, 0x80, 0xdd, 0x0e, 0x4d, 0x9e, 0x4e, 0xb1, 0xe3, 0x28, 0x4e, 0xae, 0xd8,
];

/// Nếu byte trong `data` chứa header phát hiện BitLocker (FVE volume header).
///
/// Quy tắc (theo libbde): chữ ký `-FVE-FS-` tại offset 3 (Vista format) hoặc
/// identifier GUID tại offset 0 (Windows 7/Vista/to-go format).
fn contains_bde_header(data: &[u8]) -> bool {
    if data.len() >= BDE_IDENTIFIER.len() {
        let head = &data[..BDE_IDENTIFIER.len()];
        if bytes_eq(head, &BDE_IDENTIFIER) || bytes_eq(head, &BDE_IDENTIFIER_USED_DISK_SPACE_ONLY) {
            return true;
        }
    }
    if data.len() >= 3 + BDE_SIGNATURE.len()
        && bytes_eq(&data[3..3 + BDE_SIGNATURE.len()], BDE_SIGNATURE)
    {
        return true;
    }
    false
}

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

fn file_disk_info(h: HANDLE) -> Result<DiskInfo, String> {
    let mut size: i64 = 0;
    if unsafe { GetFileSizeEx(h, &mut size) } == 0 {
        return Err("GetFileSizeEx thất bại".to_string());
    }
    let sector = 512u64; // file ảnh: mặc định 512 B/sector
    let mut sector0 = vec![0u8; 512];
    let n = read_at(h, 0, &mut sector0)?;
    let (style, mut partitions) = if n >= 512 && sector0[510] == 0x55 && sector0[511] == 0xAA {
        // MBR: chữ ký MBR tại offset 510-511
        (Style::Mbr, parse_mbr_entries(&sector0, sector))
    } else {
        let mut sector1 = vec![0u8; 512];
        let n1 = read_at(h, sector, &mut sector1)?;
        if n1 >= 512 && bytes_eq(&sector1[0..8], b"EFI PART") {
            // GPT primary header tại LBA 1
            let entry_lba = le_u64(&sector1[72..80]);
            let entry_count = le_u32(&sector1[80..84]) as usize;
            let entry_size = le_u32(&sector1[84..88]) as usize;
            let mut entries = vec![0u8; entry_count.saturating_mul(entry_size)];
            read_at(h, entry_lba * sector, &mut entries)?;
            (Style::Gpt, parse_gpt_entries(&entries, entry_size, sector))
        } else {
            (Style::File, Vec::new())
        }
    };
    // Phát hiện BitLocker: đọc header phân vùng.
    for p in partitions.iter_mut() {
        let mut head = vec![0u8; 512];
        if let Ok(n) = read_at(h, p.start_offset, &mut head) {
            if n > 0 {
                p.is_encrypted = contains_bde_header(&head[..n]);
            }
        }
    }
    Ok(DiskInfo {
        path: String::new(),
        size_bytes: size as u64,
        sector_size: sector as u32,
        style,
        partitions,
    })
}

/// Phân tích bảng MBR: 4 entry tại offset 446 (status, type, start LBA, size sectors).
fn parse_mbr_entries(s: &[u8], sector_size: u64) -> Vec<PartitionInfo> {
    let mut out = Vec::new();
    for i in 0..4usize {
        let base = 446 + i * 16;
        if base + 16 > s.len() {
            break;
        }
        let status = s[base];
        let ptype = s[base + 1];
        if ptype == 0 {
            continue;
        }
        let start_lba = le_u32(&s[base + 8..base + 12]);
        let size_sectors = le_u32(&s[base + 12..base + 16]);
        out.push(PartitionInfo {
            number: (i + 1) as u32,
            style: Style::Mbr,
            start_offset: start_lba as u64 * sector_size,
            length: size_sectors as u64 * sector_size,
            mbr_type: ptype,
            boot_indicator: status != 0,
            recognized_partition: true,
            gpt_type_guid: None,
            gpt_attributes: 0,
            gpt_name: String::new(),
            is_system: status != 0 || ptype == 0xEF,
            is_encrypted: false,
        });
    }
    out
}

/// Phân tích mảng phần tử GPT: type GUID (0 = trống), start LBA, end LBA, attrs, name.
fn parse_gpt_entries(entries: &[u8], entry_size: usize, sector_size: u64) -> Vec<PartitionInfo> {
    let mut out = Vec::new();
    if entry_size < 128 {
        return out;
    }
    let mut i = 0usize;
    while i + entry_size <= entries.len() {
        let e = &entries[i..i + entry_size];
        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&e[0..16]);
        if type_guid != [0u8; 16] {
            let start_lba = le_u64(&e[32..40]);
            let end_lba = le_u64(&e[40..48]);
            let attributes = le_u64(&e[48..56]);
            let name = parse_u16_slice(&e[56..128.min(e.len())]);
            out.push(PartitionInfo {
                number: (out.len() + 1) as u32,
                style: Style::Gpt,
                start_offset: start_lba.saturating_mul(sector_size),
                length: end_lba
                    .saturating_sub(start_lba)
                    .saturating_add(1)
                    .saturating_mul(sector_size),
                mbr_type: 0,
                boot_indicator: false,
                recognized_partition: true,
                gpt_type_guid: Some(type_guid),
                gpt_attributes: attributes,
                gpt_name: name,
                is_system: type_guid == ESP_GUID_BYTES,
                is_encrypted: false,
            });
        }
        i += entry_size;
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `EFI System Partition` GUID trên disk (ESP_GUID), định dạng byte on-disk.
/// (Xem: c12a7328-f81f-11d2-ba4b-00a0c93ec93b)
pub const ESP_GUID_BYTES: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

fn guid_bytes(g: &windows_sys::core::GUID) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&g.data1.to_le_bytes());
    out[4..6].copy_from_slice(&g.data2.to_le_bytes());
    out[6..8].copy_from_slice(&g.data3.to_le_bytes());
    out[8..16].copy_from_slice(&g.data4);
    out
}

/// Chuyển GUID (định dạng on-disk byte) thành text chuẩn xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx.
fn guid_to_string(b: &[u8]) -> String {
    if b.len() < 16 {
        return String::new();
    }
    let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let d2 = u16::from_le_bytes([b[4], b[5]]);
    let d3 = u16::from_le_bytes([b[6], b[7]]);
    format!("{d1:08x}-{d2:04x}-{d3:04x}-{}-{}", hex_str(&b[8..10]), hex_str(&b[10..16]))
}

fn hex_str(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Đọc 4/8 byte LE từ slice.
fn le_u32(b: &[u8]) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[0..4]);
    u32::from_le_bytes(a)
}

fn le_u64(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[0..8]);
    u64::from_le_bytes(a)
}

/// Giải mã chuỗi UTF-16LE (đến byte 0), dùng cho GPT name.
fn parse_u16_slice(b: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i + 1 < b.len() {
        let w = u16::from_le_bytes([b[i], b[i + 1]]);
        if w == 0 {
            break;
        }
        if let Some(c) = char::from_u32(u32::from(w)) {
            out.push(c);
        }
        i += 2;
    }
    out
}

fn gpt_name_to_string(name: &[u16; 36]) -> String {
    name.iter()
        .take_while(|&&c| c != 0)
        .filter_map(|&c| char::from_u32(u32::from(c)))
        .collect()
}

fn mbr_type_label(t: u8) -> String {
    let s = match t {
        0x00 => "Chưa phân vùng (rỗng)",
        0x01 => "FAT12 (DOS)",
        0x04 => "FAT16 (DOS 3.3+)",
        0x05 | 0x0F => "Mở rộng (extended)",
        0x06 => "FAT16 (32 MB+)",
        0x07 => "NTFS/HPFS",
        0x0B => "FAT32 (Win95 LBA)",
        0x0C => "FAT32 (Win95, LBA 65536+ cylinders)",
        0x0E => "FAT16 (LBA)",
        0x11 => "FAT16 (DOS 3.31, 2.3.1, ...)",
        0x12 => "FAT16 (DOS 4.0, 4.1, DOSX-FATX)",
        0x14 => "FAT16 (LBA, DOS 3.31+)",
        0x17 => "FAT32 (Win95, LBA 65536+ cylinders?)",
        0x1E => "FAT16 (OS/2?)",
        0x24 => "NEC DOS",
        0x27 => "FAT32 (Win95 EZ-Drive?)",
        0x3C => "PartitionMagic",
        0x40 => "Venix 80286",
        0x41 => "PowerPC/PReP boot",
        0x42 => "Windows 2000 dynamic (extended)",
        0x44 => "Acc ABOUT (Priam?)",
        0x45 => "QNX4.x (partition shares?)",
        0x50 => "OnTrack Disk Manager",
        0x51 => "OnTrack Disk Manager (duplicate?)",
        0x55 => "EZ Drive /Priam",
        0x67 => "Novell/IBM",
        0x82 => "Linux SWAP / Solaris swap",
        0x83 => "Linux (ext4/ext3/sw, XFS, ...)",
        0x84 => "Linux (LVM)",
        0x85 => "Linux extended",
        0x87 => "NTFS volume set (dynamic)",
        0x8E => "Linux LVM",
        0x9F => "BSD/OS",
        0xA0 => "IBMThinkpad utility (advanced)",
        0xA8 => "Darwin UFS",
        0xA9 => "Darwin HFS+?",
        0xAF => "HFS+ (Mac OS X)",
        0xB6 => "NTFS",
        0xBE => "Solaris boot",
        0xE1 => "Dell Utility",
        0xEB => "BSD/386",
        0xEE => "GPT protective MBR",
        0xEF => "EFI System Partition",
        0xFD => "Linux raid autodetect",
        0xFE => "Windows NT",
        _ => "Khác",
    };
    format!("{s} (0x{t:02X})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_le_u32(b: &mut [u8], v: u32) {
        b[0..4].copy_from_slice(&v.to_le_bytes());
    }

    fn put_le_u64(b: &mut [u8], v: u64) {
        b[0..8].copy_from_slice(&v.to_le_bytes());
    }

    fn put_le_u16_str(b: &mut [u8], s: &str) {
        for (i, ch) in s.chars().enumerate() {
            b[2 * i] = ch as u8;
            b[2 * i + 1] = (ch as u32 >> 8) as u8;
        }
    }

    #[test]
    fn test_contains_bde_header() {
        // Vista format: "-FVE-FS-" tại offset 3.
        let mut buf = [0u8; 512];
        buf[3..11].copy_from_slice(b"-FVE-FS-");
        assert!(contains_bde_header(&buf));
        // Windows 7/Vista format: identifier GUID tại offset 0.
        let mut buf2 = [0u8; 512];
        buf2[0..16].copy_from_slice(&super::BDE_IDENTIFIER_USED_DISK_SPACE_ONLY);
        assert!(contains_bde_header(&buf2));
        let mut buf3 = [0u8; 512];
        buf3[0..16].copy_from_slice(&[
            0x3b, 0xd6, 0x67, 0x49, 0x29, 0x2e, 0xd8, 0x4a, 0x83, 0x99, 0xf6, 0xa3, 0x39, 0xe3,
            0xd0, 0x01,
        ]);
        assert!(contains_bde_header(&buf3));
        // Không phải BitLocker.
        assert!(!contains_bde_header(&[0u8; 512]));
        let mut buf4 = [0u8; 512];
        buf4[510] = 0x55;
        buf4[511] = 0xAA; // chỉ chữ ký MBR
        assert!(!contains_bde_header(&buf4));
    }

    #[test]
    fn test_parse_mbr_entries() {
        let mut s = vec![0u8; 512];
        s[510] = 0x55;
        s[511] = 0xAA;
        // Entry 0: bootable, NTFS, start LBA 2048, size 8192.
        let base = 446;
        s[base] = 0x80;
        s[base + 1] = 0x07;
        put_le_u32(&mut s[base + 8..base + 12], 2048);
        put_le_u32(&mut s[base + 12..base + 16], 8192);
        // Entry 1: FAT32, start LBA 8192, size 4096.
        let base1 = 446 + 16;
        s[base1 + 1] = 0x0C;
        put_le_u32(&mut s[base1 + 8..base1 + 12], 8192);
        put_le_u32(&mut s[base1 + 12..base1 + 16], 4096);
        // Entry 2: empty.
        let base2 = 446 + 32;
        s[base2 + 1] = 0x00;

        let parts = parse_mbr_entries(&s, 512);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].number, 1);
        assert_eq!(parts[0].style, Style::Mbr);
        assert_eq!(parts[0].mbr_type, 0x07);
        assert_eq!(parts[0].start_offset, 2048 * 512);
        assert_eq!(parts[0].length, 8192 * 512);
        assert!(parts[0].boot_indicator);
        assert!(parts[0].is_system);
        assert_eq!(parts[1].number, 2);
        assert_eq!(parts[1].start_offset, 8192 * 512);
        assert_eq!(parts[1].length, 4096 * 512);
        assert!(!parts[1].boot_indicator);
    }

    #[test]
    fn test_parse_gpt_entries() {
        let entry_size = 128usize;
        let mut entries = vec![0u8; entry_size * 2];
        // Entry 0: ESP, start LBA 2, end LBA 33 (inclusive), name "Test ESP".
        let e = &mut entries[0..entry_size];
        e[0..16].copy_from_slice(&ESP_GUID_BYTES);
        put_le_u64(&mut e[32..40], 2);
        put_le_u64(&mut e[40..48], 33);
        put_le_u16_str(&mut e[56..72], "Test ESP");
        // Entry 1: Basic Data, start LBA 34, end LBA 100.
        let e = &mut entries[entry_size..2 * entry_size];
        e[0..16].copy_from_slice(&[
            0xBC, 0xE6, 0xA2, 0x50, 0xE3, 0x78, 0x4B, 0x8B, 0x82, 0x9F, 0x6C, 0xA5, 0x7C, 0x6E,
            0xB2, 0x3E,
        ]);
        put_le_u64(&mut e[32..40], 34);
        put_le_u64(&mut e[40..48], 100);

        let parts = parse_gpt_entries(&entries, entry_size, 512);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].style, Style::Gpt);
        assert_eq!(parts[0].gpt_type_guid, Some(ESP_GUID_BYTES));
        assert_eq!(parts[0].gpt_name, "Test ESP");
        assert!(parts[0].is_system);
        assert_eq!(parts[0].start_offset, 2 * 512);
        assert_eq!(parts[0].length, (33 - 2 + 1) * 512);
        assert_eq!(parts[1].start_offset, 34 * 512);
        assert_eq!(parts[1].length, (100 - 34 + 1) * 512);
        assert!(!parts[1].is_system);
        assert_eq!(parts[1].gpt_type_guid.unwrap()[0], 0xBC);
    }

    #[test]
    fn test_guid_to_string() {
        // ESP GUID định dạng text chuẩn.
        let s = guid_to_string(&ESP_GUID_BYTES);
        assert_eq!(s, "c12a7328-f81f-11d2-ba4b-00a0c93ec93b");
    }

    #[test]
    fn test_physical_drive_index() {
        assert_eq!(physical_drive_index("PhysicalDrive0"), Some(0));
        assert_eq!(physical_drive_index("\\\\.\\PhysicalDrive3"), Some(3));
        assert_eq!(physical_drive_index("\\\\?\\PhysicalDrive12"), Some(12));
        assert_eq!(physical_drive_index("C:\\"), None);
        assert_eq!(physical_drive_index("\\\\?\\C:\\"), None);
    }
}
