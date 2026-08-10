//! 浏览器 lazy 下载器:检查本地 → 未安装则按镜像下载 → 解压 → 写标记
use std::fs;
use std::path::Path;

use super::manifest::get_manifest;

const COMPLETE_MARKER: &str = "INSTALLATION_COMPLETE";

/// 本地浏览器目录
pub fn browser_dir(browsers_path: &str, name: &str) -> Result<String, String> {
    let entry = get_manifest(name)?;
    Ok(Path::new(browsers_path)
        .join(format!("{}-{}", entry.name, entry.revision))
        .to_string_lossy()
        .to_string())
}

/// 是否已安装
pub fn is_installed(browsers_path: &str, name: &str) -> bool {
    match browser_dir(browsers_path, name) {
        Ok(dir) => Path::new(&dir).join(COMPLETE_MARKER).exists(),
        Err(_) => false,
    }
}

/// 下载并解压浏览器
pub async fn download_browser(name: &str, browsers_path: &str, mirror: Option<&str>) -> Result<(), String> {
    if is_installed(browsers_path, name) {
        return Ok(());
    }
    let entry = get_manifest(name)?;
    let dir = browser_dir(browsers_path, name)?;
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir failed: {e}"))?;

    // 选择平台
    let dl = if cfg!(windows) {
        entry.win64.as_ref().ok_or("no win64 download")?
    } else {
        entry.linux_x64.as_ref().ok_or("no linux download")?
    };

    // 镜像列表:用户配置 > 官方
    let mirrors: Vec<String> = match mirror {
        Some(m) if !m.is_empty() => vec![m.to_string()],
        _ => dl.mirrors.clone(),
    };

    // 拼接 URL(替换占位符)
    let resolve_path = |p: &str| {
        p.replace("{browserVersion}", &entry.browser_version)
            .replace("{revision}", &entry.revision)
    };
    let resolved = resolve_path(&dl.path);

    let zip_path = Path::new(&dir).join("browser.zip");
    let mut last_err: Option<String> = None;

    for m in mirrors {
        let is_npm = m.contains("npmmirror.com");
        let prefix = if is_npm { "/binaries/playwright" } else { "" };
        let url = format!("{m}{prefix}/{resolved}");
        match download_file(&url, &zip_path).await {
            Ok(_) => {
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                let _ = fs::remove_file(&zip_path);
            }
        }
    }
    if let Some(e) = last_err {
        let _ = fs::remove_dir_all(&dir);
        return Err(format!("Download {name} failed: {e}"));
    }

    // 解压
    extract_zip(&zip_path, &dir)?;
    let _ = fs::remove_file(&zip_path);
    fs::write(Path::new(&dir).join(COMPLETE_MARKER), "").map_err(|e| e.to_string())?;
    Ok(())
}

/// 下载文件(流式写盘)
async fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    let resp = reqwest::get(url).await.map_err(|e| format!("reqwest: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("read body: {e}"))?;
    fs::write(dest, &bytes).map_err(|e| format!("write file: {e}"))
}

/// 解压 zip(Windows 用系统 tar 命令支持 zip;Linux 用 unzip)
fn extract_zip(zip_path: &Path, dest: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        // Windows 自带 tar.exe(bsdtar)支持 zip
        let out = std::process::Command::new("tar")
            .args(["-xf", &zip_path.to_string_lossy(), "-C", dest])
            .output()
            .map_err(|e| format!("tar spawn: {e}"))?;
        if !out.status.success() {
            return Err(format!("tar extract failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let out = std::process::Command::new("unzip")
            .args(["-q", &zip_path.to_string_lossy(), "-d", dest])
            .output()
            .map_err(|e| format!("unzip spawn: {e}"))?;
        if !out.status.success() {
            return Err(format!("unzip failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }
}
