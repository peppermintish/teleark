#[cfg(windows)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
fn which_rc() -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join("rc.exe").is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
fn find_windows_sdk_bin() -> Option<PathBuf> {
    if which_rc() {
        return None;
    }

    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let arch_sub = match target_arch.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "x64",
    };

    let search_roots = [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ];

    for root in search_roots {
        let base = Path::new(root);
        if let Ok(entries) = std::fs::read_dir(base) {
            let mut versions: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            // Sort descending so the latest installed Windows SDK version is preferred
            versions.sort_by(|a, b| b.cmp(a));
            for ver in versions {
                let arch_dir = ver.join(arch_sub);
                if arch_dir.join("rc.exe").is_file() {
                    return Some(arch_dir);
                }
            }
        }
    }
    None
}

fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icons/teleark.ico");
        res.set("FileDescription", "TeleArk Desktop");
        res.set("ProductName", "TeleArk");
        res.set("OriginalFilename", "teleark.exe");

        if let Some(sdk_str) = find_windows_sdk_bin().as_deref().and_then(Path::to_str) {
            res.set_toolkit_path(sdk_str);
        }

        if let Err(error) = res.compile() {
            eprintln!("cargo:warning=Failed to compile Windows PE resources: {error}");
        }
    }
}
