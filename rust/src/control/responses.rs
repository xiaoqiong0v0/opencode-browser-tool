//! 响应捕获:通过 WebView2 原生事件收集页面所有 HTTP 响应的 URL/状态码
//! 供 expect-response / assert-response 两个端点查询/断言(只读捕获,不拦截)
//! 捕获依赖 WebView2(仅 Windows);响应历史查询 clear/find 跨平台保留
use std::sync::Mutex;

use tauri::AppHandle;

/// 注册 token 存储,防止事件处理器被回收(否则响应捕获失效)
#[cfg(windows)]
static RESPONSE_TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// 响应历史(按注册顺序,最新在末尾)
static RESPONSE_LOG: Mutex<Vec<ResponseEntry>> = Mutex::new(Vec::new());

/// 简单响应记录
#[derive(Debug, Clone)]
pub struct ResponseEntry {
    pub url: String,
    pub status: i32,
}

/// 注册响应捕获(每个 page webview 创建后调用一次;仅 Windows 支持)
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
                                        unsafe { uri_out.to_hstring().to_string() }
                                    } else {
                                        String::new()
                                    };
                                    let mut code: i32 = 0;
                                    let _ = res.StatusCode(&mut code);
                                    if !url.is_empty() {
                                        // 最多保留 500 条,避免无界增长
                                        let mut log = RESPONSE_LOG.lock().unwrap();
                                        if log.len() >= 500 {
                                            log.remove(0);
                                        }
                                        log.push(ResponseEntry { url, status: code });
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

/// 清空响应历史(expect-response 调用,声明"从此开始捕获")
pub fn clear() {
    RESPONSE_LOG.lock().unwrap().clear();
}

/// 查找匹配指定 pattern(URL 子串)的响应,返回最近匹配项
pub fn find(pattern: &str) -> Option<ResponseEntry> {
    let log = RESPONSE_LOG.lock().unwrap();
    log.iter().rev().find(|e| e.url.contains(pattern)).cloned()
}