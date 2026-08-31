//! 媒体权限:WebView2 请求摄像头/麦克风权限时统一放行
//! 配合页面注入的 MEDIA_FAKE_JS 实现模拟/真实设备切换:
//! simulate 模式返回假流不占真实硬件,real 模式使用真实设备;
//! 无论哪种模式都需先放行权限,否则 real 模式拿不到真实设备、simulate 也会被 WebView2 弹权限框
use std::sync::Mutex;

use tauri::AppHandle;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
    COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    ICoreWebView2, ICoreWebView2PermissionRequestedEventArgs,
};
use webview2_com::PermissionRequestedEventHandler;

/// 保存注册 token,防止事件处理器被回收(否则权限放行失效)
static PERMISSION_TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// 注册权限放行(每个 page webview 创建后调用一次)
/// 摄像头/麦克风权限一律 Allow,避免 WebView2 权限弹窗阻塞页面 getUserMedia
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
