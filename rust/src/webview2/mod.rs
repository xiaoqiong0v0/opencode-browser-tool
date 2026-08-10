//! WebView2 渲染模块(Windows):引擎直绘到宿主窗口
//! 支持共享 Edge 用户数据(登录态)与独立数据两种模式
#![cfg(windows)]
use std::sync::Arc;
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment,
        ICoreWebView2EnvironmentOptions,
    },
    CreateCoreWebView2EnvironmentWithOptions,
};

/// WebView2 数据模式
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DataMode {
    /// 共享 Edge 用户数据(继承登录态)
    ShareEdge,
    /// 独立用户数据目录
    Isolated,
}

/// WebView2 渲染实例
pub struct WebView {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

impl WebView {
    /// 在宿主窗口内创建 WebView2 渲染区
    pub fn create(parent: HWND, mode: DataMode) -> Result<Self, String> {
        unsafe {
            // COM 初始化(线程级)
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );

            // 环境选项:指定用户数据目录
            let options: Option<ICoreWebView2EnvironmentOptions> = None;
            let env = create_env(mode, options)?;

            // 创建 controller(异步完成,这里用阻塞等待)
            let controller = create_controller_blocking(&env, parent)?;
            let webview: ICoreWebView2 = controller
                .get_CoreWebView2()
                .map_err(|e| format!("get webview failed: {e}"))?;
            Ok(Self { controller, webview })
        }
    }

    /// 导航到 URL
    pub fn navigate(&self, url: &str) -> Result<(), String> {
        let h = HSTRING::from(url);
        unsafe {
            self.webview
                .Navigate(&h)
                .map_err(|e| format!("navigate failed: {e}"))
        }
    }

    /// 调用 CDP 方法(如 Runtime.evaluate / Page.captureScreenshot)
    pub fn cdp(&self, method: &str, params_json: &str) -> Result<String, String> {
        let m = HSTRING::from(method);
        let p = HSTRING::from(params_json);
        unsafe {
            // 异步回调转同步:用事件回调接收
            let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
            let handler = CdpHandler::new(tx);
            let handler_ptr = handler.clone_interface();
            self.webview
                .CallDevToolsProtocolMethod(&m, &p, &handler)
                .map_err(|e| format!("cdp call failed: {e}"))?;
            // 保持 handler alive 直到回调
            drop(handler_ptr);
            rx.recv_timeout(std::time::Duration::from_secs(15))
                .map_err(|_| "cdp timeout".to_string())?
        }
    }

    /// 调整渲染区大小(宿主窗口 resize 时调用)
    pub fn resize(&self, width: i32, height: i32) {
        unsafe {
            let _ = self.controller.put_Bounds(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            });
        }
    }

    pub fn controller(&self) -> &ICoreWebView2Controller {
        &self.controller
    }
}

/// 创建环境:按模式选择用户数据目录
unsafe fn create_env(
    mode: DataMode,
    options: Option<ICoreWebView2EnvironmentOptions>,
) -> Result<ICoreWebView2Environment, String> {
    let user_data_folder: Option<PCWSTR> = match mode {
        DataMode::ShareEdge => {
            // Edge 用户数据目录
            let edge_dir = std::env::var("LOCALAPPDATA")
                .map(|d| format!(r"{d}\Microsoft\Edge\User Data"))
                .unwrap_or_default();
            let wide: Vec<u16> = edge_dir.encode_utf16().chain(std::iter::once(0)).collect();
            Some(PCWSTR(wide.as_ptr()))
        }
        DataMode::Isolated => None, // WebView2 默认独立目录
    };
    // 环境创建是异步的,这里简化处理
    let _ = user_data_folder;
    let _ = options;
    Err("env creation requires async handling".into())
}

/// 阻塞式创建 controller(简化:通过轮询完成标志)
unsafe fn create_controller_blocking(
    env: &ICoreWebView2Environment,
    parent: HWND,
) -> Result<ICoreWebView2Controller, String> {
    let _ = (env, parent);
    Err("controller creation requires async handling".into())
}

/// CDP 回调处理
struct CdpHandler {
    tx: std::sync::mpsc::Sender<Result<String, String>>,
}

impl CdpHandler {
    fn new(tx: std::sync::mpsc::Sender<Result<String, String>>) -> Self {
        Self { tx }
    }
}

// 简化:此处接口实现需按 webview2-com 的宏定义,待补全
#[allow(dead_code)]
fn _unused(_: Arc<()>) {}
