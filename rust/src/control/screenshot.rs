//! 截图:平台分支实现
//! Windows:通过 with_webview 拿 ICoreWebView2Controller → CDP Page.captureScreenshot
//! Linux:通过 with_webview 拿 webkit2gtk::WebView → WebKitGTK snapshot → cairo → PNG base64
//! macOS:WKWebView snapshot(待实现)

#[cfg(any(windows, target_os = "linux"))]
use std::sync::mpsc;

#[cfg(target_os = "linux")]
use base64::{engine::general_purpose::STANDARD, Engine};
#[cfg(target_os = "linux")]
use gtk::cairo;
#[cfg(target_os = "linux")]
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
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

/// Linux:WebKitGTK 快照 → cairo 图像面 → PNG base64
///
/// 区域语义:WebKitGTK 的 `WebView::snapshot` 只接受 `Visible`/`FullDocument` 两种区域、
/// 没有矩形参数;因此区域截图统一先截 `Visible`(与 Windows 分支 `captureBeyondViewport:false`
/// 一致,均为视口可见区域),再用 cairo 按视口 CSS 像素坐标裁剪,不做缩放
/// (Linux 下假定 CSS 像素与设备像素 1:1)。
///
/// 线程模型:本函数的全部调用方(service.rs / ui/annotate.rs / main.rs)都在后台线程调用。
/// `with_webview` 在非主线程只是把闭包投递到 GTK 主线程(tauri-runtime-wry 的
/// `send_user_message`,主循环里同步执行),快照完成回调也由主线程的 MainContext 触发;
/// 本线程随后阻塞在 `rx.recv_timeout` 上等待结果,不占用主循环,故双方不会互相阻塞。
#[cfg(target_os = "linux")]
fn screenshot_impl(
    app: &AppHandle,
    clip: Option<(i32, i32, i32, i32)>,
) -> Result<String, String> {
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    // 结果通道:snapshot 回调(主线程) → 本线程
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    // with_webview 在 GTK 主线程执行
    page.with_webview(move |platform_webview| {
        // PlatformWebview::inner() 在 Linux 返回 webkit2gtk::WebView
        let webview = platform_webview.inner();
        webview.snapshot(
            // 视口可见区域(非整页)
            SnapshotRegion::Visible,
            SnapshotOptions::NONE,
            // 不提供 Cancellable
            gtk::gio::Cancellable::NONE,
            move |result| {
                let _ = tx.send(match result {
                    Ok(surface) => encode_surface_png_base64(&surface, clip),
                    Err(e) => Err(format!("snapshot failed: {e}")),
                });
            },
        );
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;

    // 等待快照回调结果(10s 超时,与 Windows 分支一致)
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "screenshot timeout".to_string())?
}

/// 把 cairo 快照面编码为 PNG 并做标准 base64(可先按 CSS 像素坐标裁剪)
///
/// 参数:`surface` WebKitGTK 返回的快照面;`clip` 视口 CSS 像素选区 (x, y, w, h),
/// `None` 表示整幅可见区域
/// 返回:PNG 数据的标准 base64 字符串(不带 `data:` 前缀,与 Windows 分支输出格式一致)
#[cfg(target_os = "linux")]
fn encode_surface_png_base64(
    surface: &cairo::Surface,
    clip: Option<(i32, i32, i32, i32)>,
) -> Result<String, String> {
    // 有选区:先裁出选区新面;无选区:直接用原面
    let cropped = match clip {
        Some((x, y, w, h)) => Some(crop_surface(surface, x, y, w, h)?),
        None => None,
    };
    let source: &cairo::Surface = match cropped.as_ref() {
        Some(c) => c.as_ref(),
        None => surface,
    };
    // 由 cairo 直接写入内存流,避免落盘
    let mut png: Vec<u8> = Vec::new();
    source
        .write_to_png(&mut png)
        .map_err(|e| format!("encode png failed: {e}"))?;
    Ok(STANDARD.encode(&png))
}

/// 用 cairo 从快照面裁出指定 CSS 像素区域(1:1 设备像素,不做缩放)
///
/// 参数:`src` 源快照面;`x`/`y` 选区左上角(视口 CSS 像素,可为负);
/// `w`/`h` 选区尺寸(负值按 0 处理)
/// 返回:裁剪后的 ARGB32 图像面
#[cfg(target_os = "linux")]
fn crop_surface(
    src: &cairo::Surface,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<cairo::ImageSurface, String> {
    let (w, h) = (w.max(0), h.max(0));
    let out = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h)
        .map_err(|e| format!("create crop surface failed: {e}"))?;
    // 把源面按 (-x, -y) 偏移绘制到新面,等价于裁剪
    {
        let cr = cairo::Context::new(&out)
            .map_err(|e| format!("create cairo context failed: {e}"))?;
        cr.set_source_surface(src, -f64::from(x), -f64::from(y))
            .map_err(|e| format!("set cairo source failed: {e}"))?;
        cr.paint()
            .map_err(|e| format!("cairo paint failed: {e}"))?;
    }
    Ok(out)
}

/// macOS / 其他平台暂不支持截图
#[cfg(not(any(windows, target_os = "linux")))]
fn screenshot_impl(
    _app: &AppHandle,
    _clip: Option<(i32, i32, i32, i32)>,
) -> Result<String, String> {
    Err("screenshot not implemented on this platform yet".into())
}
