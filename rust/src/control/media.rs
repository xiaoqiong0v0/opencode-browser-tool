//! 媒体权限:页面请求摄像头/麦克风权限时统一放行
//! Windows 用 WebView2 的 PermissionRequested 事件,Linux 用 WebKitGTK 的 permission-request 信号
//! 配合页面注入的 MEDIA_FAKE_JS 实现模拟/真实设备切换:
//! simulate 模式返回假流不占真实硬件,real 模式使用真实设备;
//! 无论哪种模式都需先放行权限,否则 real 模式拿不到真实设备、simulate 也会被权限框阻塞
use tauri::AppHandle;

#[cfg(windows)]
use std::sync::Mutex;
#[cfg(windows)]
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
    COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    ICoreWebView2, ICoreWebView2PermissionRequestedEventArgs,
};
#[cfg(windows)]
use webview2_com::PermissionRequestedEventHandler;

/// 保存注册 token,防止事件处理器被回收(否则权限放行失效)
#[cfg(windows)]
static PERMISSION_TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// 注册权限放行(每个 page webview 创建后调用一次)
/// 摄像头/麦克风权限一律 Allow,避免 WebView2 权限弹窗阻塞页面 getUserMedia
#[cfg(windows)]
pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
    let _ = app;
    webview
        .with_webview(move |platform_webview| {
            unsafe {
                let controller = platform_webview.controller();
                if let Ok(wv) = controller.CoreWebView2() {
                    let handler = PermissionRequestedEventHandler::create(Box::new(
                        move |_sender: Option<ICoreWebView2>,
                              args: Option<ICoreWebView2PermissionRequestedEventArgs>|
                         -> windows::core::Result<()> {
                            if let Some(args) = args {
                                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                                // 摄像头/麦克风权限一律允许
                                if args.PermissionKind(&mut kind).is_ok()
                                    && (kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
                                        || kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE)
                                {
                                    let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW);
                                }
                            }
                            Ok(())
                        },
                    ));
                    let mut token: i64 = 0;
                    if wv.add_PermissionRequested(&handler, &mut token).is_ok() {
                        PERMISSION_TOKENS.lock().unwrap().push(token);
                    }
                }
            }
        })
        .ok();
}

/// 注册权限放行(Linux/WebKitGTK,每个 page webview 创建后调用一次)
/// 仅放行媒体类请求(摄像头/麦克风对应的 UserMediaPermissionRequest),与 Windows 只放行摄像头/麦克风语义一致;
/// 其余权限请求返回 false,保持 WebKit 默认行为;信号连接自身持有处理器,无需额外 token
#[cfg(target_os = "linux")]
pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
    use gtk::prelude::*;
    use webkit2gtk::{PermissionRequestExt, WebViewExt};
    let _ = app;
    let _ = webview.with_webview(|platform_webview| {
        let wv = platform_webview.inner();
        wv.connect_permission_request(|_wv, request| {
            // 媒体类请求(摄像头/麦克风):allow 并返回 true 表示已处理
            if let Some(req) =
                request.dynamic_cast_ref::<webkit2gtk::UserMediaPermissionRequest>()
            {
                req.allow();
                return true;
            }
            // 非媒体类权限:不强行放行,返回 false 交由 WebKit 默认行为
            false
        });
    });
}
