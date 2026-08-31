//! Windows:拦截页面新窗口请求(target=_blank 链接 / window.open)
//! WebView2 默认忽略新窗口请求(点击无效果);注册 NewWindowRequested 原生事件,
//! 拦截后在 bt-shell 打开新标签页(与工具栏"+"一致),不依赖页面 JS/IPC(远程页面 IPC 被拒)
//! 注意:页面 JS 可能同时触发 href 跳转 + window.open(同一 URL 多次请求),需过滤 about:blank 并短时间去重
use std::sync::Mutex;
use std::time::Instant;

use tauri::AppHandle;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2NewWindowRequestedEventArgs,
};
use webview2_com::NewWindowRequestedEventHandler;

/// 保存注册 token,防止事件处理器被回收(否则新窗口拦截失效)
static NEW_WINDOW_TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());
/// 上次打开的地址 + 时间(同一地址短时间去重,页面 JS 会重复触发)
static LAST_NEW_WINDOW: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// 注册新窗口拦截(每个 page webview 创建后调用一次)
pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
    let handle = app.clone();
    webview
        .with_webview(move |platform_webview| {
            unsafe {
                let controller = platform_webview.controller();
                if let Ok(wv) = controller.CoreWebView2() {
                    let handler = NewWindowRequestedEventHandler::create(Box::new(
                        move |_sender: Option<ICoreWebView2>,
                              args: Option<ICoreWebView2NewWindowRequestedEventArgs>|
                         -> windows::core::Result<()> {
                            if let Some(args) = args {
                                // 读取目标地址并标记已处理(阻止默认忽略行为)
                                let mut uri_out = windows::core::PWSTR::null();
                                let uri = if args.Uri(&mut uri_out).is_ok() {
                                    unsafe { uri_out.to_hstring().to_string() }
                                } else {
                                    String::new()
                                };
                                let _ = args.SetHandled(true);
                                // 过滤空地址/约 blank(页面 JS window.open() 占位,不应开标签)
                                if uri.is_empty() || uri == "about:blank" {
                                    return Ok(());
                                }
                                // 同一地址短时间去重(页面 JS 会 href+window.open 重复触发)
                                {
                                    let mut last = LAST_NEW_WINDOW.lock().unwrap();
                                    if let Some((u, t)) = last.as_ref() {
                                        if *u == uri && t.elapsed().as_millis() < 800 {
                                            return Ok(());
                                        }
                                    }
                                    *last = Some((uri.clone(), Instant::now()));
                                }
                                // 新标签页打开(异步,避免阻塞 WebView2 回调线程)
                                let handle = handle.clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ = crate::ui::open_new_tab(&handle, &uri);
                                });
                            }
                            Ok(())
                        },
                    ));
                    let mut token: i64 = 0;
                    if wv.add_NewWindowRequested(&handler, &mut token).is_ok() {
                        NEW_WINDOW_TOKENS.lock().unwrap().push(token);
                    }
                }
            }
        })
        .ok();
}