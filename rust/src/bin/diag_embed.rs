//! 诊断:SetParent 对 Chrome 窗口的行为
use windows::Win32::Foundation::{GetLastError, HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetParent, GetWindowLongPtrW, GetWindowThreadProcessId, SetParent,
    SetWindowLongPtrW, GWL_STYLE, WS_CHILD,
};

fn main() {
    let pid = std::env::args().nth(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    println!("target pid: {pid}");
    let mut found: HWND = HWND(std::ptr::null_mut());
    unsafe {
        let data = FindData { pid, result: &mut found };
        let _ = EnumWindows(Some(enum_proc), LPARAM(&data as *const FindData as isize));
    }
    if found.0.is_null() {
        println!("window not found");
        return;
    }
    println!("found hwnd: {found:?}");
    unsafe {
        // 先设 WS_CHILD 样式
        let style = GetWindowLongPtrW(found, GWL_STYLE) as u32;
        println!("style before: 0x{style:08X}");
        let _ = SetWindowLongPtrW(found, GWL_STYLE, (style | WS_CHILD.0) as isize);
        // 用 test host 窗口做父窗口
        let host = windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
            windows::core::w!("STATIC"),
            windows::core::w!("diag-host"),
            windows::Win32::UI::WindowsAndMessaging::WS_OVERLAPPEDWINDOW,
            0, 0, 400, 300,
            None, None, None, None,
        ).unwrap();
        let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
            host,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOW,
        );
        let r = SetParent(found, Some(host));
        let err = GetLastError();
        println!("SetParent result: {r:?}, last error: {err:?}");
        let parent = GetParent(found);
        println!("parent after: {parent:?} (host: {host:?})");
        std::thread::sleep(std::time::Duration::from_secs(3));
        let parent2 = GetParent(found);
        println!("parent after 3s: {parent2:?} (host: {host:?})");
    }
}

struct FindData {
    pid: u32,
    result: *mut HWND,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let data = &*(lparam.0 as *const FindData);
    let mut pid = 0u32;
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid != data.pid {
        return windows::core::BOOL(1);
    }
    let mut class_buf = [0u16; 64];
    unsafe {
        let n = GetClassNameW(hwnd, &mut class_buf);
        let class_name = String::from_utf16_lossy(&class_buf[..n as usize]);
        if !class_name.contains("Chrome_WidgetWin") {
            return windows::core::BOOL(1);
        }
    }
    unsafe {
        *data.result = hwnd;
    }
    windows::core::BOOL(0)
}
