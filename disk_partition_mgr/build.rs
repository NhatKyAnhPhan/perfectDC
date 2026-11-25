//! Nhúng icon vào file EXE trên Windows (nếu tìm thấy rc.exe của Windows SDK).
//!
//! Không bắt buộc: nếu thiếu rc.exe, quá trình build vẫn tiếp tục và chỉ
//! hiển thị cảnh báo; ứng dụng vẫn có icon cửa sổ nạp lúc chạy.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("assets").join("icon.ico");
    if !icon.exists() {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=Không tìm thấy rc.exe — bỏ qua nhúng icon vào EXE.");
        return;
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc_file = out_dir.join("perfectds.rc");
    let res_file = out_dir.join("perfectds.res");
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "1.0.0".into());
    let mut nums: Vec<u16> = version
        .split('.')
        .map(|p| p.parse::<u16>().unwrap_or(0))
        .collect();
    nums.resize(4, 0);
    let filever = nums
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let rc_text = format!(
        r#"1 ICON "assets/icon.ico"

1 VERSIONINFO
FILEVERSION {filever}
PRODUCTVERSION {filever}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "PerfectDS"
      VALUE "FileDescription", "PerfectDS - Trinh quan ly & chia o dia"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "PerfectDS"
      VALUE "ProductName", "PerfectDS"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    // Dùng đường dẫn tương đối (rc.exe đọc file .rc theo bảng mã ANSI, nên
    // đường dẫn tuyệt đối chứa tiếng Việt có thể bị lỗi "file not found").
    if fs::write(&rc_file, rc_text).is_err() {
        println!("cargo:warning=Không ghi được file .rc — bỏ qua icon.");
        return;
    }

    let status = Command::new(&rc)
        .current_dir(&manifest)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status();

    match status {
        Ok(s) if s.success() && res_file.exists() => {
            println!("cargo:rustc-link-arg={}", res_file.display());
        }
        Ok(s) => println!(
            "cargo:warning=rc.exe kết thúc với mã {:?} — bỏ qua nhúng icon.",
            s.code()
        ),
        Err(e) => println!("cargo:warning=Không chạy được rc.exe ({e}) — bỏ qua nhúng icon."),
    }
}

fn find_rc() -> Option<PathBuf> {
    if let Some(p) = env::var_os("RC") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Ok(out) = Command::new("where.exe").arg("rc.exe").output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) {
            let p = PathBuf::from(line);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    let mut roots = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = env::var_os(var) {
            roots.push(
                PathBuf::from(base)
                    .join("Windows Kits")
                    .join("10")
                    .join("bin"),
            );
        }
    }
    for base in [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ] {
        roots.push(PathBuf::from(base));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            for arch in ["x64", "x86", "arm64"] {
                let rc = p.join(arch).join("rc.exe");
                if rc.is_file() {
                    candidates.push(rc);
                }
            }
        }
    }
    // Ưu tiên SDK mới nhất (tên thư mục phiên bản sắp xếp tăng dần).
    candidates.sort();
    candidates.pop().or_else(find_vs_rc)
}

fn find_vs_rc() -> Option<PathBuf> {
    let mut roots = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = env::var_os(var) {
            roots.push(PathBuf::from(base).join("Microsoft Visual Studio"));
        }
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for root in roots {
        collect_vs(&root, 0, &mut found);
    }
    found.sort();
    found.pop()
}

fn collect_vs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let rc = p.join("bin").join("Hostx64").join("x64").join("rc.exe");
            if rc.is_file() {
                out.push(rc);
            }
            collect_vs(&p, depth + 1, out);
        }
    }
}
