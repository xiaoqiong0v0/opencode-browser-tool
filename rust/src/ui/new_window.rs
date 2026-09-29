//! 拦截页面新窗口请求(target=_blank 链接 / window.open),改为本项目新标签页打开
//! Windows:WebView2 默认忽略新窗口请求(点击无效果),注册 NewWindowRequested 原生事件拦截;
//! Linux:WebKitGTK 需要新窗口时触发 create 信号,返回 None 拒绝 WebKit 自行创建窗口。
//! 两者均在拦截后调用 bt-shell 的"新建标签"逻辑(与工具栏"+"一致),不依赖页面 JS/IPC(远程页面 IPC 被拒)
//! 注意:页面 JS 可能同时触发 href 跳转 + window.open(同一 URL 多次请求),需过滤 about:blank 并短时间去重
use std::sync::Mutex;
use std::time::Instant;

use tauri::AppHandle;

#[cfg(windows)]
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2NewWindowRequestedEventArgs,
};
#[cfg(windows)]
use webview2_com::NewWindowRequestedEventHandler;

/// 保存注册 token,防止事件处理器被回收(否则新窗口拦截失效)
#[cfg(windows)]
static NEW_WINDOW_TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());
/// 上次打开的地址 + 时间(同一地址短时间去重,页面 JS 会重复触发)
static LAST_NEW_WINDOW: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// 判断是否应忽略本次新窗口请求:空地址/about:blank(占位),或同一地址 800ms 内重复
/// 参数:uri 请求的目标地址
/// 返回值:true 表示忽略(不下发新标签)
fn should_skip(uri: &str) -> bool {
    if uri.is_empty() || uri == "about:blank" {
        return true;
    }
    let mut last = LAST_NEW_WINDOW.lock().unwrap();
    if let Some((u, t)) = last.as_ref() {
        if *u == uri && t.elapsed().as_millis() < 800 {
            return true;
        }
    }
    *last = Some((uri.to_string(), Instant::now()));
    false
}

/// 注册新窗口拦截(每个 page webview 创建后调用一次)
#[cfg(windows)]
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
                                // 过滤空地址/about:blank 与同一地址短时重复
                                if should_skip(&uri) {
                                    return Ok(());
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

/// 注册新窗口拦截(Linux/WebKitGTK,每个 page webview 创建后调用一次)
/// create 信号仅在需要新窗口时触发;返回 None 拒绝 WebKit 自行创建窗口,
/// 改由本项目 open_new_tab 打开(与 Windows 同一调用链);信号连接自身持有处理器,无需 token
#[cfg(target_os = "linux")]
pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
    use webkit2gtk::{URIRequestExt, WebViewExt};
    let handle = app.clone();
    let _ = webview.with_webview(move |platform_webview| {
        let wv = platform_webview.inner();
        wv.connect_create(move |_wv, action| {
            // create 信号给出导航动作,从中取本次新窗口请求的目标地址
            let uri = action
                .request()
                .and_then(|req| req.uri())
                .map(|uri| uri.to_string())
                .unwrap_or_default();
            // 过滤空地址/about:blank 与同一地址短时重复
            if should_skip(&uri) {
                return None;
            }
            // 新标签页打开(异步,避免阻塞 GTK 信号回调)
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                let _ = crate::ui::open_new_tab(&handle, &uri);
            });
            // 返回 None:拒绝 WebKit 自行创建新窗口
            None
        });
    });
}
