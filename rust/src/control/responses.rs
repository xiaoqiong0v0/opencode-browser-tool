//! 响应捕获:收集页面所有 HTTP 响应的 URL/状态码
//! 供 expect-response / assert-response 两个端点查询/断言(只读捕获,不拦截)
//! Windows 走 WebView2 WebResourceResponseReceived 事件,Linux 走 WebKitGTK resource-load-started + notify::response;
//! 响应历史查询 clear/find 跨平台保留
use std::sync::Mutex;

use tauri::AppHandle;

/// 注册 token 存储,防止事件处理器被回收(否则响应捕获失效)
#[cfg(windows)]
static RESPONSE_TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// 响应历史(按注册顺序,最新在末尾)
static RESPONSE_LOG: Mutex<Vec<ResponseEntry>> = Mutex::new(Vec::new());

/// 响应历史上限(超出丢弃最旧记录,避免无界增长)
const MAX_RESPONSES: usize = 500;

/// 简单响应记录
#[derive(Debug, Clone)]
pub struct ResponseEntry {
    pub url: String,
    pub status: i32,
}

/// 追加一条响应记录(最多保留 MAX_RESPONSES 条,最新在末尾)
fn push_entry(entry: ResponseEntry) {
    let mut log = RESPONSE_LOG.lock().unwrap();
    if log.len() >= MAX_RESPONSES {
        log.remove(0);
    }
    log.push(entry);
}

/// 从 WebKitWebResource 读出 (url, status);响应未就绪或 URL 为空时返回 None(不写入空条目)
#[cfg(target_os = "linux")]
fn read_response(resource: &webkit2gtk::WebResource) -> Option<ResponseEntry> {
    use webkit2gtk::{URIResponseExt, WebResourceExt};

    let response = resource.response()?;
    let url = response.uri().map(|u| u.to_string()).unwrap_or_default();
    if url.is_empty() {
        return None;
    }
    Some(ResponseEntry { url, status: response.status_code() as i32 })
}

/// 注册响应捕获(Windows/WebView2,每个 page webview 创建后调用一次)
#[cfg(windows)]
pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
    use windows::core::Interface;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2_2, ICoreWebView2WebResourceResponseReceivedEventArgs,
    };
    use webview2_com::WebResourceResponseReceivedEventHandler;

    let _ = app;
    webview
        .with_webview(move |platform_webview| {
            unsafe {
                let controller = platform_webview.controller();
                if let Ok(wv) = controller.CoreWebView2() {
                    // 响应事件在 ICoreWebView2_2 及以上接口上
                    let Ok(wv2) = wv.cast::<ICoreWebView2_2>() else {
                        return;
                    };
                    let handler = WebResourceResponseReceivedEventHandler::create(Box::new(
                        move |_sender: Option<ICoreWebView2>,
                              args: Option<ICoreWebView2WebResourceResponseReceivedEventArgs>|
                         -> windows::core::Result<()> {
                            if let Some(args) = args {
                                // 读取请求 URL 与响应状态码
                                if let (Ok(req), Ok(res)) = (args.Request(), args.Response()) {
                                    let mut uri_out = windows::core::PWSTR::null();
                                    let url = if req.Uri(&mut uri_out).is_ok() {
                                        uri_out.to_hstring().to_string()
                                    } else {
                                        String::new()
                                    };
                                    let mut code: i32 = 0;
                                    let _ = res.StatusCode(&mut code);
                                    if !url.is_empty() {
                                        push_entry(ResponseEntry { url, status: code });
                                    }
                                }
                            }
                            Ok(())
                        },
                    ));
                    let mut token: i64 = 0;
                    if wv2.add_WebResourceResponseReceived(&handler, &mut token).is_ok() {
                        RESPONSE_TOKENS.lock().unwrap().push(token);
                    }
                }
            }
        })
        .ok();
}

/// 注册响应捕获(Linux/WebKitGTK,每个 page webview 创建后调用一次)
/// resource-load-started 拿到 WebKitWebResource 时响应头通常尚未到达,
/// 故对其监听 notify::response,响应到达后再读出 URIResponse 的 uri/status_code 写入历史。
/// 信号处理器由 GObject 自身持有(对象存活期间长期有效),且 resource 在加载期间由 WebKit 持有,
/// 因此无需 Windows 那样的额外 token 集合。
#[cfg(target_os = "linux")]
pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
    use gtk::prelude::*;
    use webkit2gtk::WebViewExt;

    let _ = app;
    let _ = webview.with_webview(|platform_webview| {
        let wv = platform_webview.inner();
        wv.connect_resource_load_started(|_wv, resource, _request| {
            // 响应已就绪(极少见)时直接记录;否则等 notify::response 触发
            if let Some(entry) = read_response(resource) {
                push_entry(entry);
                return;
            }
            resource.connect_notify_local(Some("response"), |resource, _pspec| {
                if let Some(entry) = read_response(resource) {
                    push_entry(entry);
                }
            });
        });
    });
}

/// 清空响应历史(expect-response 调用,声明"从此开始捕获")
pub fn clear() {
    RESPONSE_LOG.lock().unwrap().clear();
}

/// 查找匹配指定 pattern(URL 子串)的响应,返回最近匹配项
pub fn find(pattern: &str) -> Option<ResponseEntry> {
    let log = RESPONSE_LOG.lock().unwrap();
    log.iter().rev().find(|e| e.url.contains(pattern)).cloned()
}