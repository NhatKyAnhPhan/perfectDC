# disk_partition_mgr — kiểm tra và mô phỏng thao tác quản lý phân vùng đĩa (Windows)

CLI:
- `inspect [--disk <đĩa>]` — hiển thị kích thước đĩa, sector size, danh sách phân vùng, vùng chưa phân bổ; hoặc liệt kê các đĩa vật lý.
- `plan-move --disk <đĩa> --partition <n> --direction <left|right> --offset-sectors <n> [--dry-run] [--apply]` — tính kế hoạch dời phân vùng (căn chỉnh theo sector size, kiểm tra va chạm/biên giới, cảnh báo BitLocker / phân vùng hệ thống).

**Mặc định luôn là dry-run** — tuyệt đối không ghi đĩa thật trừ khi truyền `--apply`.

## Build

```
cargo build --release
```

## Ví dụ

```
target\release\disk_partition_mgr.exe inspect
target\release\disk_partition_mgr.exe inspect --disk \\?\C:\test\disk.vhd
target\release\disk_partition_mgr.exe plan-move --disk \\?\PhysicalDrive0 --partition 2 --direction right --offset-sectors 16 --dry-run
target\release\disk_partition_mgr.exe plan-move --disk \\?\PhysicalDrive0 --partition 2 --direction right --offset-sectors 16 --apply
```

## Tạo file `.vhd` test để chạy thử an toàn

Cần quyền admin và module Hyper-V (PowerShell):

```powershell
# 1. Tạo VHD
New-VHD -Path 'D:\test\dpm_test.vhd' -Dynamic -SizeBytes 40MB

# 2. Mount và khởi tạo bảng phân vùng
Mount-VHD -Path 'D:\test\dpm_test.vhd' -PassThru
$disk = Get-Disk -UniqueId (Get-VHD -Path 'D:\test\dpm_test.vhd').UniqueId
Initialize-Disk -InputObject $disk -PartitionStyle MBR

# 3. Tạo 2 phân vùng test
New-Partition -DiskNumber $disk.Number -Size 4194304
New-Partition -DiskNumber $disk.Number -Size 8388608
```

Kiểm tra bằng tool:

```
target\release\disk_partition_mgr.exe inspect --disk \\?\PhysicalDrive<N>
```

(`<N>` là số ổ đĩa ảo mới trong `Get-Disk`.) Nếu module Hyper-V không khả dụng, dùng file ảnh bất kỳ có bảng phân vùng MBR/GPT (`inspect --disk <file>` đọc bảng phân vùng từ byte file; ví dụ `D:\test\dpm_test.vhd` được tạo trong quá trình test).

## Lưu ý bảo mật / rủi ro

- **Không ghi khi chưa `--apply`**: chỉ `--apply` (với kiểm tra lại va chạm/biên giới trước khi ghi) mới có tác động ghi.
- **Đĩa vật lý**: mở `\\.\PhysicalDriveN` qua `CreateFileW` + `DeviceIoControl(IOCTL_DISK_GET_DRIVE_LAYOUT_EX / IOCTL_DISK_GET_DRIVE_GEOMETRY_EX)` cần quyền admin và privilege như `SeReadFilePrivilege`/`SeBackupPrivilege`/`SeRestorePrivilege` — môi trường như container/VM có thể từ chối; chạy PowerShell với quyền admin.
- **BitLocker**: phát hiện qua chữ ký `-FVE-FS-` và identifier GUID của FVE volume header (theo libbde); cảnh báo khi di chuyển phân vùng đã mã hóa — copy dữ liệu trên BitLocker có thể làm mất dữ liệu hoặc vô hiệu hóa BitLocker.
- **Phân vùng hệ thống**: cảnh báo khi phân vùng ESP/boot (ESP GUID) hoặc MBR BootIndicator — thao tát trên phân vùng khởi động có thể phá vỡ khả năng khởi động.
- **Thay đổi bảng phân vùng**: `plan-move` chỉ di chuyển dữ liệu trong phạm vi phân vùng hiện có — thêm/xoá phân vùng không được hỗ trợ.
- Nếu đĩa có BitLocker, không dùng `--apply` trừ khi biết rõ rủi ro.
