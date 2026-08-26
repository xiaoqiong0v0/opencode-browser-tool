//! 截图:平台分支实现
//! Windows:通过 with_webview 拿 ICoreWebView2Controller → CDP Page.captureScreenshot
//! Linux/macOS:WebKit/WKWebView snapshot(待实现)
use std::sync::mpsc;
use tauri::AppHandle;

use crate::ui;

/// 截图页面 Webview,返回 PNG base64
pub fn screenshot(app: &AppHandle) -> Result<String, String> {
    screenshot_impl(app, None)
}

/// 截取页面指定区域(视口 CSS 像素坐标),返回 PNG base64
pub fn screenshot_clip(
    app: &AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<String, String> {
    screenshot_impl(app, Some((x, y, w, h)))
}

#[cfg(windows)]
fn screenshot_impl(
    app: &AppHandle,
    clip: Option<(i32, i32, i32, i32)>,
) -> Result<String, String> {
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    // 结果通道:completed 回调 → 本线程
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let tx2 = tx.clone();
    // with_webview 在 UI 线程执行
    page.with_webview(move |platform_webview| {
        // PlatformWebview::controller() 返回 ICoreWebView2Controller(与 tauri 同版本 webview2-com 0.38)
        let controller = platform_webview.controller();
        unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use windows::core::Interface;

            let webview_result: Result<ICoreWebView2, String> = controller
                .CoreWebView2()
                .map_err(|e| format!("get webview failed: {e}"));
            match webview_result {
                Err(e) => { let _ = tx.send(Err(e)); }
                Ok(webview) => {
                    // 区域截图:CDP clip 用视口 CSS 像素坐标,scale=1 按 device 像素输出
                    let params = match clip {
                        Some((cx, cy, cw, ch)) => format!(
                            r#"{{"format":"png","captureBeyondViewport":false,"clip":{{"x":{cx},"y":{cy},"width":{cw},"height":{ch},"scale":1}}}}"#
                        ),
                        None => r#"{"format":"png","captureBeyondViewport":false}"#.to_string(),
                    };
                    let method = windows::core::HSTRING::from("Page.captureScreenshot");
                    let params = windows::core::HSTRING::from(params);
                    // 在 UI 线程泵消息等待回调
                    // 回调签名 (windows::core::Result<()>, String) → windows::core::Result<()>
                    let result = CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
                        Box::new(move |handler| {
                            webview
                                .CallDevToolsProtocolMethod(&method, &params, &handler)
                                .map_err(webview2_com::Error::from)
                        }),
                        Box::new(
                            move |_result: windows::core::Result<()>, json: String| -> windows::core::Result<()> {
                                let _ = tx2.send(Ok(json));
                                Ok(())
                            },
                        ),
                    );
                    if let Err(e) = result {
                        let _ = tx.send(Err(e.to_string()));
                    }
                }
            }
        }
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;

    // 等待回调结果(10s 超时)
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "screenshot timeout".to_string())?
        .and_then(|json| {
            // CDP 返回 { "data": "<base64>", ... }
            let v: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| format!("parse cdp result: {e}"))?;
            v.get("data")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| format!("no data in cdp result: {json}"))
        })
}

#[cfg(not(windows))]
fn screenshot_impl(
    _app: &AppHandle,
    _clip: Option<(i32, i32, i32, i32)>,
) -> Result<String, String> {
    Err("screenshot not implemented on this platform yet".into())
}
