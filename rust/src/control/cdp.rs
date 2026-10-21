//! Windows CDP 调用通道(webview2 `ICoreWebView2::CallDevToolsProtocolMethod` 封装)
//! 复用方:截图(screenshot.rs)、可访问性树(accessibility.rs)、键盘注入(keyboard.rs)
//! 说明:回调返回的 jsonResult 即 CDP 的 result 对象本身(不带 id/result 信封)
//!
//! 等待策略(1.9.0 修复「iframe_click 卡死」的根因):
//! webview2-com 的 `wait_for_async_operation` 内部走 `wait_with_pump` → `GetMessage` **无限阻塞**等消息,
//! 且会 `DispatchMessage` 把消息派回 UI 线程。CDP 完成回调本就由 WebView2 派往 UI 线程,因此:
//! - 在 `with_webview`(持有 UI 线程)内调它 → 回调无处可派 → **死锁**,整个浏览器卡死;
//! - 消息流被污染(嵌套派发/外部 WM_QUIT)时,`GetMessage` 也永不返回 → 主线程永久卡住。
//! 故此处**不使用** `wait_for_async_operation`:只在 `with_webview` 内发起调用,结果等待放回**外部阻塞线程**,
//! 用**有界** `recv_timeout` 轮询(回调仍由 WebView2 派到 UI 线程,tauri 主循环正常泵消息即可送达)。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::ui;

/// CDP 调用总超时(ms):覆盖 `CallDevToolsProtocolMethod` 发起到回调送达的全过程
const CDP_TIMEOUT_MS: u64 = 10_000;
/// 轮询间隔(ms):外层线程每隔多久检查一次结果/超时(短轮询保证即使踢消息失败也能按时返回)
const CDP_POLL_MS: u64 = 15;

/// 调用任意 CDP 方法并同步等待其结果 JSON(默认超时 `CDP_TIMEOUT_MS`)
pub fn call_json(app: &AppHandle, method_name: &str, params_json: &str) -> Result<String, String> {
    call_json_timeout(app, method_name, params_json, CDP_TIMEOUT_MS)
}

/// 同 `call_json`,但由调用方指定超时(ms)——模型 `--timeout` 的 CDP 侧入口
/// 编排多步的链路(上传/可访问性/键盘等)**每一步**都用自己的预算等待,总最坏 ≈ 步数 × 单步超时
pub fn call_json_timeout(
    app: &AppHandle,
    method_name: &str,
    params_json: &str,
    timeout_ms: u64,
) -> Result<String, String> {
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let tx2 = tx.clone();
    let method_str = method_name.to_string();
    let params_str = params_json.to_string();
    // 只在 with_webview 内**发起**调用,不在此等待结果(见文件头注释:嵌套等待会死锁 UI 线程)
    page.with_webview(move |platform_webview| {
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
                    let method = windows::core::HSTRING::from(method_str.as_str());
                    let params = windows::core::HSTRING::from(params_str.as_str());
                    // create 只创建回调对象(不阻塞);回调由 WebView2 派到 UI 线程执行,
                    // 内部 send 结果 —— 等待发生在下方 wait_bounded(外部线程),不嵌套泵消息
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

    // 有界等待:结果/超时都明确返回,不无限阻塞(对齐项目 ready:false 式诚实口径)
    wait_bounded(rx, timeout_ms)
}

/// 内部统一入口:`timeout_ms = None` 走默认(与 1.8.x 完全一致),`Some(ms)` 用传入预算
pub(crate) fn call_json_opt(
    app: &AppHandle,
    method_name: &str,
    params_json: &str,
    timeout_ms: Option<u64>,
) -> Result<String, String> {
    match timeout_ms {
        Some(ms) => call_json_timeout(app, method_name, params_json, ms),
        None => call_json(app, method_name, params_json),
    }
}

/// 有界等待 CDP 结果:短间隔 `recv_timeout` 轮询,直到拿到结果或总超时
/// 参数:`rx` 结果通道;`timeout_ms` 总超时毫秒
/// 返回:拿到结果则透传 Ok/Err;超时/通道断开返回带明确原因的 Err(不静默、不无限等)
/// 复用方:`cdp::call_json`、`screenshot` 的 Windows CDP 路径(二者都不在 `with_webview` 内等待)
pub(crate) fn wait_bounded(
    rx: mpsc::Receiver<Result<String, String>>,
    timeout_ms: u64,
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match rx.recv_timeout(Duration::from_millis(CDP_POLL_MS)) {
            Ok(v) => return v,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "cdp timeout: no response within {timeout_ms}ms (UI thread busy or callback never delivered)"
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("cdp channel disconnected: completion callback was dropped".into());
            }
        }
    }
}
