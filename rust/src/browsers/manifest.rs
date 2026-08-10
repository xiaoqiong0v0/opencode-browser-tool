//! 浏览器静态清单(与 Node 版 scripts/gen-browsers-manifest.mjs 生成内容一致)
//! 镜像规则: {mirror}/builds/cft/{browserVersion}/{platform}/{file}(CFT)
//!           {mirror}/builds/{name}/{revision}/{file}(旧格式)

pub struct DownloadEntry {
    pub path: String,
    pub mirrors: Vec<String>,
}

pub struct BrowserEntry {
    pub name: String,
    pub revision: String,
    pub browser_version: String,
    pub win64: Option<DownloadEntry>,
    pub linux_x64: Option<DownloadEntry>,
}

const OFFICIAL_MIRRORS: [&str; 3] = [
    "https://cdn.playwright.dev/dbazure/download/playwright",
    "https://playwright.download.prss.microsoft.com/dbazure/download/playwright",
    "https://cdn.playwright.dev",
];

pub fn get_manifest(name: &str) -> Result<BrowserEntry, String> {
    match name {
        "chromium" => Ok(BrowserEntry {
            name: "chromium".into(),
            revision: "1228".into(),
            browser_version: "149.0.7827.55".into(),
            win64: Some(DownloadEntry {
                path: "builds/cft/{browserVersion}/win64/chrome-win64.zip".into(),
                mirrors: OFFICIAL_MIRRORS.iter().map(|s| s.to_string()).collect(),
            }),
            linux_x64: Some(DownloadEntry {
                path: "builds/cft/{browserVersion}/linux64/chrome-linux64.zip".into(),
                mirrors: OFFICIAL_MIRRORS.iter().map(|s| s.to_string()).collect(),
            }),
        }),
        "firefox" => Ok(BrowserEntry {
            name: "firefox".into(),
            revision: "1532".into(),
            browser_version: "151.0".into(),
            win64: Some(DownloadEntry {
                path: "builds/firefox/{revision}/firefox-win64.zip".into(),
                mirrors: OFFICIAL_MIRRORS.iter().map(|s| s.to_string()).collect(),
            }),
            linux_x64: Some(DownloadEntry {
                path: "builds/firefox/{revision}/firefox-ubuntu-24.04.zip".into(),
                mirrors: OFFICIAL_MIRRORS.iter().map(|s| s.to_string()).collect(),
            }),
        }),
        _ => Err(format!("Unknown browser: {name}")),
    }
}
