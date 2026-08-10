//! 覆盖层窗口:透明、点击穿透,绘制高亮框和批注标记
#![cfg(windows)]
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, SelectObject, PS_SOLID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    RegisterClassW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, LAYERED_WINDOW_ATTRIBUTES_FLAGS, MSG,
    SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};

/// 高亮矩形
#[derive(Debug, Clone, Copy)]
pub struct HighlightRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub color: u32, // 0x00RRGGBB
}

/// 覆盖层:透明点击穿透窗口,绘制高亮
pub struct Overlay {
    hwnd: HWND,
    /// 高亮数据(窗口过程通过 GWLP_USERDATA 访问)
    highlights: std::sync::Arc<std::sync::Mutex<Vec<HighlightRect>>>,
}

impl Overlay {
    pub fn create(parent: HWND) -> Result<Self, String> {
        unsafe {
            let class_name = w!("pw_shell_overlay");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_wnd_proc),
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                lpszClassName: class_name,
                style: CS_HREDRAW | CS_VREDRAW,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let ex_style = WINDOW_EX_STYLE(
                WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0,
            );
            let hwnd = CreateWindowExW(
                ex_style,
                class_name,
                w!("pw_overlay"),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                Some(wc.hInstance),
                None,
            )
            .map_err(|e| format!("overlay create failed: {e}"))?;

            let highlights: std::sync::Arc<std::sync::Mutex<Vec<HighlightRect>>> =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            // 存 Arc 指针到窗口 user data(需保持 alive)
            let ptr = std::sync::Arc::into_raw(highlights.clone()) as isize;
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr);

            // 全透明层
            let _ = SetLayeredWindowAttributes(
                hwnd,
                COLORREF(0),
                255,
                LAYERED_WINDOW_ATTRIBUTES_FLAGS(2), // LWA_ALPHA
            );
            let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Ok(Self { hwnd, highlights })
        }
    }

    /// 设置高亮并重绘
    pub fn set_highlights(&self, rects: Vec<HighlightRect>) {
        *self.highlights.lock().unwrap() = rects;
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    /// 调整到父窗口客户区大小并定位
    pub fn resize_to_parent(&self, parent: HWND) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(parent, &mut rc);
            let w = rc.right - rc.left;
            let h = rc.bottom - rc.top;
            let _ = SetWindowPos(
                self.hwnd,
                Some(parent),
                0,
                0,
                w,
                h,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW,
            );
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            // 释放 user data 中的 Arc
            let ptr = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA);
            if ptr != 0 {
                let _ =
                    std::sync::Arc::from_raw(ptr as *const std::sync::Mutex<Vec<HighlightRect>>);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
            if ptr != 0 {
                let highlights = unsafe { &*(ptr as *const std::sync::Mutex<Vec<HighlightRect>>) };
                if let Ok(rects) = highlights.lock() {
                    for r in rects.iter() {
                        let color = COLORREF(r.color);
                        let brush = CreateSolidBrush(color);
                        let pen = CreatePen(PS_SOLID, 2, color);
                        let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
                        let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
                        let rc = RECT {
                            left: r.x,
                            top: r.y,
                            right: r.x + r.w,
                            bottom: r.y + r.h,
                        };
                        FillRect(hdc, &rc, brush);
                        let _ = SelectObject(hdc, old_brush);
                        let _ = SelectObject(hdc, old_pen);
                        let _ = DeleteObject(HGDIOBJ(brush.0));
                        let _ = DeleteObject(HGDIOBJ(pen.0));
                    }
                }
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
