//! 截图:平台分支实现
//! Windows:通过 with_webview 拿 ICoreWebView2Controller → CDP Page.captureScreenshot
//! Linux:通过 with_webview 拿 webkit2gtk::WebView → WebKitGTK snapshot → cairo → PNG base64
//! macOS:WKWebView snapshot(待实现)
//! 两条路径各有常规(视口 clip)与越界(整块截全,元素比视口还大)两种模式,见各函数注释

#[cfg(any(windows, target_os = "linux"))]
use std::sync::mpsc;

#[cfg(target_os = "linux")]
use base64::{Engine, engine::general_purpose::STANDARD};
#[cfg(target_os = "linux")]
use gtk::cairo;
use tauri::AppHandle;
#[cfg(target_os = "linux")]
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

use crate::ui;

/// 截图等待总超时(ms,默认):发起截图到结果送达的全过程有界;模型 `--timeout` 可覆盖
/// 1.9.0 批次4:10_000 → 15_000(与 CDP/eval/wait_for_selector 统一为 15s)
#[cfg(any(windows, target_os = "linux"))]
const SCREENSHOT_TIMEOUT_MS: u64 = 15_000;

/// 截图页面 Webview,返回 PNG base64
pub fn screenshot(app: &AppHandle) -> Result<String, String> {
    screenshot_impl(app, None, false, None)
}

/// 同 `screenshot`,`timeout_ms` 为 Some 时用传入预算(整条截图链:滚动偏移 eval + CDP/快照等待)
pub fn screenshot_with_timeout(app: &AppHandle, timeout_ms: u64) -> Result<String, String> {
    screenshot_impl(app, None, false, Some(timeout_ms))
}

/// 截取页面指定区域(**视口** CSS 像素坐标),返回 PNG base64
///
/// 内部统一换算成文档坐标再交给平台实现(CDP clip 实测恒为文档坐标,见 `screenshot_impl` 注释)
pub fn screenshot_clip(app: &AppHandle, x: i32, y: i32, w: i32, h: i32) -> Result<String, String> {
    screenshot_impl(app, Some((x, y, w, h)), false, None)
}

/// 同 `screenshot_clip`,`timeout_ms` 为 Some 时用传入预算
pub fn screenshot_clip_with_timeout(
    app: &AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    timeout_ms: u64,
) -> Result<String, String> {
    screenshot_impl(app, Some((x, y, w, h)), false, Some(timeout_ms))
}

/// 越界截取页面指定区域(**视口** CSS 像素坐标),返回 PNG base64
///
/// 用于元素比视口还高/宽的场景(rect 无法完全落进视口),平台实现:
/// - Windows:CDP `Page.captureScreenshot` + `captureBeyondViewport:true`(内部换算为文档坐标)
/// - Linux:WebKitGTK `SnapshotRegion::FullDocument` 整页快照后按文档坐标裁剪
pub fn screenshot_clip_beyond(
    app: &AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<String, String> {
    screenshot_impl(app, Some((x, y, w, h)), true, None)
}

/// 同 `screenshot_clip_beyond`,`timeout_ms` 为 Some 时用传入预算
pub fn screenshot_clip_beyond_with_timeout(
    app: &AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    timeout_ms: u64,
) -> Result<String, String> {
    screenshot_impl(app, Some((x, y, w, h)), true, Some(timeout_ms))
}

/// 读取窗口滚动偏移(整像素),用于视口坐标 → 文档坐标换算
///
/// 实测依据(v1.7.0):CDP `Page.captureScreenshot` 的 `clip` **恒为文档坐标**
/// (`captureBeyondViewport` true/false 均如此),视口坐标传入会截到文档靠顶部的错误区域
#[cfg(any(windows, target_os = "linux"))]
fn scroll_offset(app: &AppHandle, timeout_ms: Option<u64>) -> Result<(i32, i32), String> {
    let v = super::eval_with(
        app,
        "(function(){return {x:window.scrollX,y:window.scrollY};})()",
        timeout_ms,
    )?;
    Ok((
        v.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0).round() as i32,
        v.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0).round() as i32,
    ))
}

/// 平台分支实现入口
///
/// 参数:`clip` 截图选区 (x, y, w, h),**视口 CSS 像素**坐标(平台实现内换算为文档坐标),
/// `None` 表示整幅视口;`beyond` 为 true 时走越界路径(元素比视口还大,整块截全);
/// `timeout_ms` = Some 时用模型传入预算,否则沿用默认 `SCREENSHOT_TIMEOUT_MS`
#[cfg(windows)]
fn screenshot_impl(
    app: &AppHandle,
    clip: Option<(i32, i32, i32, i32)>,
    beyond: bool,
    timeout_ms: Option<u64>,
) -> Result<String, String> {
    let wait_ms = timeout_ms.unwrap_or(SCREENSHOT_TIMEOUT_MS);
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    // CDP clip 恒为**文档坐标**(实测,与 captureBeyondViewport 取值无关) → 视口坐标先换算
    let clip = match clip {
        Some((x, y, w, h)) => {
            let (sx, sy) = scroll_offset(app, timeout_ms)?;
            Some((x + sx, y + sy, w, h))
        }
        None => None,
    };
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
                    // 区域截图(clip 已换算为文档坐标;实测见文件头 scroll_offset 注释):
                    // - captureBeyondViewport:false → 仅截视口表面(clip 须完全落在可见视口内)
                    // - captureBeyondViewport:true → 越界整块截全(元素比视口还大时用)
                    let params = match clip {
                        Some((cx, cy, cw, ch)) => format!(
                            r#"{{"format":"png","captureBeyondViewport":{beyond},"clip":{{"x":{cx},"y":{cy},"width":{cw},"height":{ch},"scale":1}}}}"#
                        ),
                        None => format!(r#"{{"format":"png","captureBeyondViewport":{beyond}}}"#),
                    };
                    let method = windows::core::HSTRING::from("Page.captureScreenshot");
                    let params = windows::core::HSTRING::from(params);
                    // 只发起、不在此等待:旧实现 wait_for_async_operation 在 UI 线程内 GetMessage 无限阻塞
                    // (与 cdp.rs 同根因 → 浏览器可被永久卡死);结果等待改到下方 wait_bounded(有界)
                    let completed = CallDevToolsProtocolMethodCompletedHandler::create(
                        Box::new(
                            move |_result: windows::core::Result<()>, json: String| {
                                let _ = tx2.send(Ok(json));
                                Ok(())
                            },
                        ),
                    );
                    if let Err(e) =
                        webview.CallDevToolsProtocolMethod(&method, &params, &completed)
                    {
                        let _ = tx.send(Err(format!("CallDevToolsProtocolMethod failed: {e}")));
                    }
                }
            }
        }
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;

    // 等待回调结果(有界:超时返回明确错误,不无限阻塞)再解析 CDP 的 { "data": "<base64>" }
    super::cdp::wait_bounded(rx, wait_ms).and_then(|json| {
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
/// 没有矩形参数;因此:
/// - 常规区域(`beyond=false`):截 `Visible`(视口可见区域,与 Windows 分支
///   `captureBeyondViewport:false` 对应),裁剪用**视口** CSS 像素(快照原点=视口左上角);
/// - 越界区域(`beyond=true`,元素比视口还高/宽):截 `FullDocument` 整页快照,
///   裁剪前先把视口坐标换算成**文档**坐标(快照原点=文档 (0,0),与 Windows 的 CDP
///   clip 文档坐标口径一致)。
///
/// HiDPI 单位换算(1.10.0 修复):视口/文档坐标都是 **CSS 像素**,而 WebKitGTK 快照面
/// 按**设备像素**输出(HiDPI 下 = CSS × 缩放),旧实现 1:1 裁剪 → 高 DPI 上裁错位置/尺寸。
/// 现在裁剪前统一乘 `k`(webview 缩放因子),换算公式:
/// `设备像素 = round(CSS 像素 × k)`(x/y/w/h 各自换算,非整数缩放四舍五入)。
///
/// 线程模型:本函数的全部调用方(service.rs / ui/annotate.rs / main.rs)都在后台线程调用。
/// `with_webview` 在非主线程只是把闭包投递到 GTK 主线程(tauri-runtime-wry 的
/// `send_user_message`,主循环里同步执行),快照完成回调也由主线程的 MainContext 触发;
/// 本线程随后阻塞在 `rx.recv_timeout` 上等待结果,不占用主循环,故双方不会互相阻塞。
#[cfg(target_os = "linux")]
fn screenshot_impl(
    app: &AppHandle,
    clip: Option<(i32, i32, i32, i32)>,
    beyond: bool,
    timeout_ms: Option<u64>,
) -> Result<String, String> {
    let wait_ms = timeout_ms.unwrap_or(SCREENSHOT_TIMEOUT_MS);
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    // 越界路径裁剪用文档坐标(FullDocument 原点=文档 0,0);常规路径保持视口坐标(Visible 原点=视口)
    let clip = match clip {
        Some((x, y, w, h)) if beyond => {
            let (sx, sy) = scroll_offset(app, timeout_ms)?;
            Some((x + sx, y + sy, w, h))
        }
        other => other,
    };
    // 结果通道:snapshot 回调(主线程) → 本线程
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    // with_webview 在 GTK 主线程执行
    page.with_webview(move |platform_webview| {
        // PlatformWebview::inner() 在 Linux 返回 webkit2gtk::WebView
        let webview = platform_webview.inner();
        // CSS → 设备像素的换算系数:取 widget 缩放(即快照面的渲染缩放);
        // 分数缩放用 f64 承接,裁剪时四舍五入到整像素。异常值(<=0/非有限)回退 1.0
        let scale = {
            let s = f64::from(gtk::prelude::WidgetExt::scale_factor(&webview));
            if s.is_finite() && s > 0.0 { s } else { 1.0 }
        };
        webview.snapshot(
            // 越界:整页快照(原点=文档 0,0);常规:视口可见区域
            if beyond {
                SnapshotRegion::FullDocument
            } else {
                SnapshotRegion::Visible
            },
            SnapshotOptions::NONE,
            // 不提供 Cancellable
            gtk::gio::Cancellable::NONE,
            move |result| {
                let _ = tx.send(match result {
                    Ok(surface) => encode_surface_png_base64(&surface, clip, scale),
                    Err(e) => Err(format!("snapshot failed: {e}")),
                });
            },
        );
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;

    // 等待快照回调结果(有界:默认 15s,可被模型 `--timeout` 覆盖;与 Windows 分支一致)
    rx.recv_timeout(std::time::Duration::from_millis(wait_ms))
        .map_err(|_| format!("screenshot timeout (budget {wait_ms}ms)"))?
}

/// 把 cairo 快照面编码为 PNG 并做标准 base64(可先按 CSS 像素坐标裁剪)
///
/// 参数:`surface` WebKitGTK 返回的快照面(设备像素);`clip` 选区 (x, y, w, h) ——
/// 坐标系随快照区域而定(常规=视口 CSS 像素/Visible 快照,越界=文档 CSS 像素/FullDocument 快照),
/// `None` 表示整幅快照不裁剪;`scale` CSS → 设备像素的换算系数(HiDPI 缩放)
/// 返回:PNG 数据的标准 base64 字符串(不带 `data:` 前缀,与 Windows 分支输出格式一致)
#[cfg(target_os = "linux")]
fn encode_surface_png_base64(
    surface: &cairo::Surface,
    clip: Option<(i32, i32, i32, i32)>,
    scale: f64,
) -> Result<String, String> {
    // 有选区:先裁出选区新面;无选区:直接用原面
    let cropped = match clip {
        Some((x, y, w, h)) => Some(crop_surface(surface, x, y, w, h, scale)?),
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

/// CSS 像素 → 设备像素并四舍五入(HiDPI 裁剪的坐标换算)
///
/// 参数:`css` CSS 像素值(可为负);`scale` 缩放系数(<=0/非有限按 1.0 处理)
/// 返回:`round(css × scale)`
#[cfg(target_os = "linux")]
fn css_to_device(css: i32, scale: f64) -> i32 {
    let k = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (f64::from(css) * k).round() as i32
}

/// 用 cairo 从快照面裁出指定区域(选区为 CSS 像素,内部换算为设备像素)
///
/// 参数:`src` 源快照面(设备像素);`x`/`y` 选区左上角 CSS 像素(坐标系与快照面一致:
/// 常规=视口,越界=文档;可为负);`w`/`h` 选区尺寸 CSS 像素(负值按 0 处理);
/// `scale` CSS → 设备像素的换算系数
/// 返回:裁剪后的 ARGB32 图像面(尺寸 = 选区 CSS 尺寸 × scale,四舍五入)
#[cfg(target_os = "linux")]
fn crop_surface(
    src: &cairo::Surface,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    scale: f64,
) -> Result<cairo::ImageSurface, String> {
    let (w, h) = (
        css_to_device(w.max(0), scale),
        css_to_device(h.max(0), scale),
    );
    let (dx, dy) = (css_to_device(x, scale), css_to_device(y, scale));
    let out = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h)
        .map_err(|e| format!("create crop surface failed: {e}"))?;
    // 把源面按 (-dx, -dy) 偏移绘制到新面,等价于裁剪(均为设备像素)
    {
        let cr =
            cairo::Context::new(&out).map_err(|e| format!("create cairo context failed: {e}"))?;
        cr.set_source_surface(src, -f64::from(dx), -f64::from(dy))
            .map_err(|e| format!("set cairo source failed: {e}"))?;
        cr.paint().map_err(|e| format!("cairo paint failed: {e}"))?;
    }
    Ok(out)
}

/// macOS / 其他平台暂不支持截图
#[cfg(not(any(windows, target_os = "linux")))]
fn screenshot_impl(
    _app: &AppHandle,
    _clip: Option<(i32, i32, i32, i32)>,
    _beyond: bool,
    _timeout_ms: Option<u64>,
) -> Result<String, String> {
    Err("screenshot not implemented on this platform yet".into())
}
