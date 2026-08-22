//! 设备预设:内置常用设备模式(窗口尺寸 + User-Agent)
//! 应用时:窗口 set_size 触发 Resized → ui::apply_layout 自动重排
//! UA 运行时修改仅 Windows 支持(ICoreWebView2Settings2.SetUserAgent),其他平台返回错误
use tauri::{AppHandle, LogicalSize};

use crate::ui;

/// 设备预设(名称唯一)
pub struct Device {
    pub name: &'static str,
    pub width: f64,
    pub height: f64,
    pub ua: Option<&'static str>,
}

/// 内置设备预设表
pub const DEVICES: &[Device] = &[
    Device { name: "desktop-1080p", width: 1920.0, height: 1080.0, ua: None },
    Device { name: "desktop-1440p", width: 1440.0, height: 900.0, ua: None },
    Device { name: "ipad-pro-11", width: 834.0, height: 1194.0, ua: Some("Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1") },
    Device { name: "ipad-10", width: 820.0, height: 1180.0, ua: Some("Mozilla/5.0 (iPad; CPU OS 16_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.4 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-15-pro", width: 393.0, height: 852.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-14", width: 390.0, height: 844.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 16_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.4 Mobile/15E148 Safari/604.1") },
    Device { name: "iphone-se", width: 375.0, height: 667.0, ua: Some("Mozilla/5.0 (iPhone; CPU iPhone OS 15_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Mobile/15E148 Safari/604.1") },
    Device { name: "pixel-7", width: 412.0, height: 915.0, ua: Some("Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36") },
];

/// 按名称查找设备
pub fn find(name: &str) -> Option<&'static Device> {
    DEVICES.iter().find(|d| d.name == name)
}

/// 应用设备预设:设置窗口尺寸(逻辑像素) + 运行时修改 UA
pub fn apply(app: &AppHandle, device: &Device) -> Result<(), String> {
    // 窗口尺寸(逻辑像素,set_size 自动换算物理尺寸并触发 Resized 重排)
    let win = ui::page_webview(app)
        .ok_or("page webview not ready")?
        .window_ref()
        .clone();
    win.set_size(LogicalSize::new(device.width, device.height))
        .map_err(|e| format!("set window size failed: {e}"))?;

    // 运行时修改 UA(仅 Windows)
    if let Some(ua) = device.ua {
        set_user_agent(app, ua)?;
    }
    Ok(())
}

/// 运行时修改页面 Webview 的 User-Agent
/// Windows:通过 controller → ICoreWebView2Settings2.SetUserAgent
#[cfg(windows)]
fn set_user_agent(app: &AppHandle, ua: &str) -> Result<(), String> {
    use std::sync::mpsc;
    let page = ui::page_webview(app).ok_or("page webview not ready")?;
    // 结果通道:闭包内执行(闭包须 'static,不能捕获局部变量)
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let ua_owned = ua.to_string();
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

/// 非 Windows 平台暂不支持运行时修改 UA
#[cfg(not(windows))]
fn set_user_agent(_app: &AppHandle, _ua: &str) -> Result<(), String> {
    Err("runtime user-agent change not supported on this platform yet".into())
}
