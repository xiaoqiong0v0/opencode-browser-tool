//! 外壳窗口模块(Windows):宿主窗口 + 浏览器嵌入 + 透明覆盖层
//! Linux 版本在 M5 实现(x11rb),此处为 Windows 实现
#![cfg(windows)]
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, EndPaint, FillRect, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    RegisterClassW, SetParent, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, MSG, SW_SHOW, WINDOW_EX_STYLE, WM_DESTROY, WM_PAINT, WM_SIZE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

pub struct ShellWindow {
    hwnd: HWND,
}

impl ShellWindow {
    /// 创建宿主窗口(浏览器嵌入区 + 覆盖层)
    pub fn create(title: &str, width: i32, height: i32) -> Result<Self, String> {
        unsafe {
            let class_name = w!("pw_shell_host");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(host_wnd_proc),
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                lpszClassName: class_name,
                style: CS_HREDRAW | CS_VREDRAW,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let title_pcwstr = windows::core::PCWSTR(title_wide.as_ptr());
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                title_pcwstr,
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width,
                height,
                None,
                None,
                Some(wc.hInstance),
                None,
            )
            .map_err(|e| format!("CreateWindowExW failed: {e}"))?;
            ShowWindow(hwnd, SW_SHOW);
            Ok(Self { hwnd })
        }
    }

    /// 将外部窗口(浏览器)嵌入外壳
    pub fn embed(&self, child: HWND) {
        unsafe {
            let _ = SetParent(child, Some(self.hwnd));
        }
    }

    /// 嵌入后把子窗口定位到客户区并铺满
    pub fn layout_child(&self, child: HWND) {
        unsafe {
            let mut rc = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(self.hwnd, &mut rc);
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                child,
                None,
                0,
                0,
                rc.right - rc.left,
                rc.bottom - rc.top,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW,
            );
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 运行消息循环(阻塞,需在专用线程)
    pub fn message_loop(&self) {
        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }
    }
}

impl Drop for ShellWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// 宿主窗口过程:处理尺寸变化 → 重新布局嵌入的浏览器
unsafe extern "system" fn host_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let brush: HBRUSH = CreateSolidBrush(COLORREF(0x00202020));
            FillRect(hdc, &ps.rcPaint, brush);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as i32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as i32;
            if let Some(child) = get_child(hwnd) {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    child,
                    None,
                    0,
                    0,
                    width,
                    height,
                    windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                );
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn get_child(hwnd: HWND) -> Option<HWND> {
    unsafe {
        let child = windows::Win32::UI::WindowsAndMessaging::GetWindow(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GW_CHILD,
        );
        match child {
            Ok(c) if !c.is_invalid() => Some(c),
            _ => None,
        }
    }
}



