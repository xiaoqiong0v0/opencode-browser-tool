//! 设备预设:内置常用设备模式(**内容区(视口)尺寸** + User-Agent)
//! 应用时:窗口 set_size(内容区高 + 工具栏) 触发 Resized → ui::apply_layout 自动重排
//! UA 运行时修改:Windows 用 ICoreWebView2Settings2.SetUserAgent,Linux 用 WebKitSettings.set_user_agent,其他平台返回错误
//! UA 语义(1.9.0 起统一):`ua=Some` → 覆盖;`ua=None` → **恢复默认 UA**(桌面预设与「默认」项同此,不再跳过)
use tauri::{AppHandle, LogicalSize};

use crate::ui;

/// 设备预设(名称唯一);width/height 为**内容区(视口)**尺寸(逻辑像素)
pub struct Device {
    pub name: &'static str,
    pub width: f64,
    pub height: f64,
    pub ua: Option<&'static str>,
}

/// 「默认」预设的特殊名:面板首项/CLI 传此名 → 恢复默认 UA + 内容区回默认尺寸
/// 不进 DEVICES 表(`/api/device/list` 不返回它),面板首项「默认」由前端写死此值
pub const DEFAULT_DEVICE_NAME: &str = "__default__";

/// 默认内容区尺寸(逻辑像素):窗口 = 628 + 72(工具栏) = 700,与启动窗口 ui 内建 1100x700 同口径
pub const DEFAULT_CONTENT_W: f64 = 1100.0;
pub const DEFAULT_CONTENT_H: f64 = 628.0;

/// 「默认」预设:恢复默认 UA + 默认内容区(仅按 DEFAULT_DEVICE_NAME 命中,不参与设备列表)
pub const DEFAULT_DEVICE: Device = Device {
    name: DEFAULT_DEVICE_NAME,
    width: DEFAULT_CONTENT_W,
    height: DEFAULT_CONTENT_H,
    ua: None,
};

/// 内置设备预设表(首项 desktop-720p 为默认设备);宽高即内容区(视口)尺寸
pub const DEVICES: &[Device] = &[
    Device { name: "desktop-720p", width: 1024.0, height: 720.0, ua: None },
    Device { name: "desktop-1080p", width: 1920.0, height: 1080.0, ua: None },
    Device { name: "desktop-900p", width: 1440.0, height: 900.0, ua: None },
    Device { name: "ipad-pro-11", width: 834.0, height: 1194.0, ua: Some("Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1") },
    Device { name: "ipad-10", width: 820.0, height: 1180.0, ua: Some("Mozilla/5.0 (iPad; CPU OS 16_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.4 Mobile/15E148 Safari/604.1") },
    Device { name: "ipad-mini-6", width: 744.0, height: 1133.0, ua: Some("Mozilla/5.0 (iPad; CPU OS 15_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-15-pro", width: 393.0, height: 852.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-14", width: 390.0, height: 844.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 16_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.4 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-13", width: 390.0, height: 844.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 15_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-12", width: 390.0, height: 844.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 14_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/14.1 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-se", width: 375.0, height: 667.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 15_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Mobile/15E148 Safari/604.1") },
    Device { name: "pixel-7", width: 412.0, height: 915.0, ua: Some("Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36") },
    Device { name: "pixel-6", width: 411.0, height: 914.0, ua: Some("Mozilla/5.0 (Linux; Android 13; Pixel 6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36") },
    Device { name: "galaxy-s23", width: 360.0, height: 780.0, ua: Some("Mozilla/5.0 (Linux; Android 13; SM-S911B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/111.0.0.0 Mobile Safari/537.36") },
];

/// 按名称查找设备(含特殊名 DEFAULT_DEVICE_NAME;设备列表只遍历 DEVICES,不含「默认」)
pub fn find(name: &str) -> Option<&'static Device> {
    if name == DEFAULT_DEVICE_NAME {
        return Some(&DEFAULT_DEVICE);
    }
    DEVICES.iter().find(|d| d.name == name)
}

/// 应用设备预设:设置窗口尺寸(**内容区高 + 工具栏**,逻辑像素) + 运行时处理 UA
/// UA 语义统一:`device.ua = Some` → 覆盖;`None` → 恢复默认 UA(1.9.0 起不再跳过,
/// 否则桌面预设/「默认」项会保留上一个移动 UA,输出标 (default) 却是假的)
pub fn apply(app: &AppHandle, device: &Device) -> Result<(), String> {
    // 窗口 = 内容区宽 x (内容区高 + 工具栏高);set_size 自动换算物理尺寸并触发 Resized 重排
    let win = ui::active_page_webview(app)
        .ok_or("page webview not ready")?
        .window_ref()
        .clone();
    let win_h = device.height + ui::TOOLBAR_HEIGHT;
    win.set_size(LogicalSize::new(device.width, win_h))
        .map_err(|e| format!("set window size failed: {e}"))?;

    // UA 必被处理(覆盖或恢复);失败向上报错,不静默
    set_user_agent(app, device.ua)?;
    Ok(())
}

/// 运行时修改/恢复页面 Webview 的 User-Agent
/// - `Some(ua)`:覆盖为指定 UA(首次覆盖前先缓存当前默认 UA,供恢复用)
/// - `None`:恢复默认 UA = 写回缓存的默认 UA;从未覆盖则 no-op(当前已是默认)
/// Windows:通过 controller → ICoreWebView2Settings2.SetUserAgent
/// 注:**不能**用 `SetUserAgent("")` 恢复 —— 1.9.0 实测空串不回系统默认(仍报上一个覆盖值),
/// 因此走「首次覆盖前缓存默认 UA,恢复时写回」
#[cfg(windows)]
pub fn set_user_agent(app: &AppHandle, ua: Option<&str>) -> Result<(), String> {
    use std::sync::mpsc;
    // 恢复默认:读缓存;从未覆盖(None)→ no-op,不发任何设置调用
    let ua_owned = match ua {
        Some(v) => {
            capture_default_ua(app)?;
            v.to_string()
        }
        None => match cached_default_ua() {
            Some(v) => v,
            None => return Ok(()),
        },
    };
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    // 结果通道:闭包内执行(闭包须 'static,不能捕获局部变量)
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    page.with_webview(move |platform_webview| {
        use windows::core::Interface;
        unsafe {
            let controller = platform_webview.controller();
            let webview = match controller.CoreWebView2() {
                Ok(w) => w,
                Err(e) => { let _ = tx.send(Err(format!("get webview failed: {e}"))); return; }
            };
            let settings = match webview.Settings() {
                Ok(s) => s,
                Err(e) => { let _ = tx.send(Err(format!("get settings failed: {e}"))); return; }
            };
            // ICoreWebView2Settings2 提供 UserAgent 属性(运行时修改)
            match settings.cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings2>() {
                Ok(settings2) => {
                    let ua_wide = windows::core::HSTRING::from(&ua_owned);
                    match settings2.SetUserAgent(&ua_wide) {
                        Ok(()) => { let _ = tx.send(Ok(())); }
                        Err(e) => { let _ = tx.send(Err(format!("set user agent failed: {e}"))); }
                    }
                }
                Err(e) => { let _ = tx.send(Err(format!("settings2 cast failed: {e}"))); }
            }
        }
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "set user agent timeout".to_string())?
}

/// 运行时修改/恢复页面 Webview 的 User-Agent
/// Linux:通过当前激活页面 webview 的 WebKitSettings::set_user_agent(仅影响该 webview,与 Windows 语义一致)
/// `None` → `set_user_agent(None)` 即回退 WebKit 默认 UA
#[cfg(target_os = "linux")]
pub fn set_user_agent(app: &AppHandle, ua: Option<&str>) -> Result<(), String> {
    use std::sync::mpsc;
    use webkit2gtk::{SettingsExt, WebViewExt};
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    // 结果通道:闭包内执行(闭包须 'static,不能捕获局部变量)
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let ua_owned = ua.map(str::to_string);
    page.with_webview(move |platform_webview| {
        let widget = platform_webview.inner();
        match widget.settings() {
            Some(settings) => {
                // set_user_agent 接受 Option<&str>,None 表示回退默认 UA
                settings.set_user_agent(ua_owned.as_deref());
                let _ = tx.send(Ok(()));
            }
            None => {
                let _ = tx.send(Err("get webkit settings failed".into()));
            }
        }
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "set user agent timeout".to_string())?
}

/// 其余平台暂不支持运行时修改 UA
#[cfg(not(any(windows, target_os = "linux")))]
pub fn set_user_agent(_app: &AppHandle, _ua: Option<&str>) -> Result<(), String> {
    Err("runtime user-agent change not supported on this platform yet".into())
}

// ---- 私有:Windows 默认 UA 缓存(恢复时写回;Linux 用 WebKit 原生 None,不需要) ----

/// 进程级缓存:**首次覆盖前**抓到的默认 UA(None = 还没覆盖过)
/// 存在原因:实测 `SetUserAgent("")` 不会恢复 WebView2 系统默认(仍报上一个覆盖值),
/// 只能记住覆盖前的值、恢复时写回;从未覆盖 → 恢复为 no-op
#[cfg(windows)]
static DEFAULT_UA: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 覆盖前抓取一次当前页面 UA 作为「默认 UA」并缓存(已有缓存则跳过)
/// 必须在 `with_webview` 设置**之前**单独执行:eval 同样走 with_webview,嵌套会死锁
#[cfg(windows)]
fn capture_default_ua(app: &AppHandle) -> Result<(), String> {
    let mut guard = DEFAULT_UA
        .lock()
        .map_err(|e| format!("default ua lock failed: {e}"))?;
    if guard.is_some() {
        return Ok(());
    }
    // 缓存先写入、后 SetUserAgent:并发覆盖时第二个调用读到的仍是首次覆盖前的值
    let cur = crate::control::evaluate(app, "navigator.userAgent")?
        .as_str()
        .ok_or_else(|| "default ua read failed: non-string result".to_string())?
        .to_string();
    *guard = Some(cur);
    Ok(())
}

/// 读取缓存的默认 UA(None = 从未覆盖 → 恢复时 no-op)
#[cfg(windows)]
fn cached_default_ua() -> Option<String> {
    DEFAULT_UA.lock().ok().and_then(|g| g.clone())
}
