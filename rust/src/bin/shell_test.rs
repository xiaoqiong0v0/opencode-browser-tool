//! M3 测试:外壳窗口 + 浏览器嵌入 + 覆盖层高亮
use pw_shell::cdp::browser::{BrowserOptions, CdpBrowser};
use pw_shell::shell::host::ShellWindow;
use pw_shell::shell::overlay::{HighlightRect, Overlay};
use std::time::Duration;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, GetWindowTextW,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("== M3 shell test ==");
    // 1. 创建外壳窗口(消息循环在创建线程运行)
    let host = ShellWindow::create("pw-shell host", 1000, 700)?;
    println!("host window created: {:?}", host.hwnd());

    // 2. 在后台线程启动浏览器
    let browser_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut browser = CdpBrowser::new();
        let exe = "C:/Users/king-/.opencode/plugins-data/opencode-playwright-tool/browsers/chromium-1228/chrome-win64/chrome.exe";
        rt.block_on(async {
            browser
                .launch(
                    &BrowserOptions {
                        executable: exe.into(),
                        user_data_dir: "C:/Users/king-/AppData/Local/Temp/opencode/m3-profile".into(),
                        headless: false,
                    },
                    "https://example.com",
                )
                .await
        })?;
        println!("browser launched");
        Ok::<CdpBrowser, String>(browser)
    });
    let browser = browser_handle.join().unwrap().map_err(|e| e)?;
    println!("browser launched (pid {:?})", browser.pid());

    // 3. 枚举顶层窗口:找 Chrome_WidgetWin_1 类 + 有标题的窗口(--app 主窗口)
    let mut found: HWND = HWND(std::ptr::null_mut());
    unsafe {
        let data = FindData { result: &mut found };
        let _ = EnumWindows(Some(enum_proc), LPARAM(&data as *const FindData as isize));
    }
    if found.0.is_null() {
        println!("WARN: browser window not found");
    } else {
        println!("browser hwnd: {found:?}");
        host.embed(found);
        // 嵌入后定位到外壳客户区
        host.layout_child(found);
        println!("embedded + positioned!");
        // 4. 创建覆盖层并绘制高亮
        let overlay = Overlay::create(host.hwnd())?;
        overlay.resize_to_parent(host.hwnd());
        overlay.set_highlights(vec![
            HighlightRect { x: 100, y: 100, w: 200, h: 40, color: 0x0000FF00 },
            HighlightRect { x: 350, y: 200, w: 150, h: 60, color: 0x00FF0000 },
            HighlightRect { x: 50, y: 300, w: 300, h: 30, color: 0x0000FFFF },
        ]);
        println!("overlay highlights drawn (green/red/yellow boxes)");
        std::mem::forget(overlay);
    }

    // 5. 运行消息循环(阻塞,观察窗口)
    println!("host window running (close window to exit)...");
    host.message_loop();
    println!("== done ==");
    Ok(())
}

struct FindData {
    result: *mut HWND,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let data = &*(lparam.0 as *const FindData);
    // 窗口类必须是 Chrome 主窗口
    let mut class_buf = [0u16; 64];
    unsafe {
        let n = GetClassNameW(hwnd, &mut class_buf);
        let class_name = String::from_utf16_lossy(&class_buf[..n as usize]);
        if !class_name.contains("Chrome_WidgetWin") {
            return BOOL(1);
        }
    }
    // 有标题(主窗口)
    let mut title_buf = [0u16; 256];
    unsafe {
        let n = GetWindowTextW(hwnd, &mut title_buf);
        if n == 0 {
            return BOOL(1);
        }
    }
    unsafe {
        *data.result = hwnd;
    }
    BOOL(0)
}
