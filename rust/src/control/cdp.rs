//! Windows CDP 调用通道(webview2 `ICoreWebView2::CallDevToolsProtocolMethod` 封装)
//! 复用方:截图(screenshot.rs)、可访问性树(accessibility.rs)、键盘注入(keyboard.rs)
//! 说明:回调返回的 jsonResult 即 CDP 的 result 对象本身(不带 id/result 信封)

use std::sync::mpsc;

use tauri::AppHandle;

use crate::ui;

/// 调用任意 CDP 方法并同步等待其结果 JSON
///
/// 参数:`app` Tauri 应用句柄;`method_name` CDP 方法名(如 `"Input.dispatchKeyEvent"`);
/// `params_json` CDP 参数对象的 JSON 字符串
/// 返回:CDP result 对象的 JSON 字符串(通常是 `"{}"`);Webview 未就绪/调用失败/超时返回 Err
pub fn call_json(app: &AppHandle, method_name: &str, params_json: &str) -> Result<String, String> {
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let tx2 = tx.clone();
    let method_str = method_name.to_string();
    let params_str = params_json.to_string();
    page.with_webview(move |platform_webview| {
        let controller = platform_webview.controller();
        unsafe {
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;

            let webview_result: Result<ICoreWebView2, String> = controller
                .CoreWebView2()
                .map_err(|e| format!("get webview failed: {e}"));
            match webview_result {
                Err(e) => { let _ = tx.send(Err(e)); }
                Ok(webview) => {
                    let method = windows::core::HSTRING::from(method_str.as_str());
                    let params = windows::core::HSTRING::from(params_str.as_str());
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

    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "cdp timeout".to_string())?
}
