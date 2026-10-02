//! Linux(WebKitGTK/GDK)可信输入通道:`gdk_event_put` 注入原生键盘/指针事件
//!
//! 机制:在 GTK 主线程(`webview.with_webview`)把合成 `GdkEvent` 投入 GDK 事件队列,
//! 由 GTK 分发给 `WebKitWebView` → 页面收到 `isTrusted=true` 的原生事件,默认行为生效。
//!
//! WSLg 实测(见 `.tmp/scratch/probe*.out`):单字符输入、Enter 提交、逐字符输入、
//! 鼠标点击命中(带坐标)、CSS `:hover` 全部生效且 `isTrusted=true`。
//! 仅 Linux 编译。
#![cfg(target_os = "linux")]

use std::sync::mpsc;

use gtk::gdk;
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use tauri::AppHandle;

use super::ControlResult;
use crate::ui;

/// 待注入的一条事件(键盘 Key 会展开为按下 + 抬起)
pub enum Inj {
    /// 键盘按键,`keyval` 为 GDK keysym
    Key(u32),
    /// 指针移动到 (x, y)(webview 视口 CSS 像素)
    Move(f64, f64),
    /// 鼠标按键 (x, y, button, press)
    Button(f64, f64, u32, bool),
}

/// 在页面 Webview 上按顺序注入事件(GTK 主线程执行,阻塞至完成/5s 超时)
pub fn inject(app: &AppHandle, events: Vec<Inj>) -> ControlResult<()> {
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    let (tx, rx) = mpsc::channel::<ControlResult<()>>();
    page.with_webview(move |pw| {
        let view = pw.inner();
        let r = inject_all(&view, &events);
        let _ = tx.send(r);
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "gdk inject timeout".to_string())?
}

/// GTK 主线程:取 webview 的 GdkWindow,逐条投递事件
fn inject_all(view: &webkit2gtk::WebView, events: &[Inj]) -> ControlResult<()> {
    // 确保 webview 持有 GTK 焦点,否则按键无人接收
    view.grab_focus();
    let win = view.window().ok_or("webview has no GdkWindow")?;
    // 事件窗口若是 toplevel 窗口(webview 无独立 GdkWindow),坐标需加上 webview 原点
    let (offx, offy) = window_offset(view, &win);
    let keymap = gdk::Keymap::for_display(&view.display());
    for ev in events {
        match ev {
            Inj::Key(keyval) => {
                let code = keymap
                    .as_ref()
                    .and_then(|km| km.entries_for_keyval(*keyval).into_iter().next())
                    .map(|k| k.keycode())
                    .unwrap_or(0);
                put_key(&win, *keyval, code, true)?;
                put_key(&win, *keyval, code, false)?;
            }
            Inj::Move(x, y) => put_motion(&win, x + offx, y + offy)?,
            Inj::Button(x, y, button, press) => {
                put_button(&win, x + offx, y + offy, *button, *press)?
            }
        }
    }
    Ok(())
}

/// 事件窗口坐标系下 webview 原点的偏移
///
/// `win` 是 `view.window()`:若它就是 toplevel 的 GdkWindow(webview 无独立窗口),
/// 则返回 webview 在 toplevel 内的原点;否则(webview 有独立窗口)返回 (0, 0)。
fn window_offset(view: &webkit2gtk::WebView, win: &gdk::Window) -> (f64, f64) {
    if let Some(tl) = view.toplevel() {
        if let Some(tlw) = tl.window() {
            if tlw == *win {
                if let Some((dx, dy)) = view.translate_coordinates(&tl, 0, 0) {
                    return (f64::from(dx), f64::from(dy));
                }
            }
        }
    }
    (0.0, 0.0)
}

/// 构造并投递一次键盘事件
fn put_key(win: &gdk::Window, keyval: u32, keycode: u32, press: bool) -> ControlResult<()> {
    let type_ = if press {
        gdk::EventType::KeyPress
    } else {
        gdk::EventType::KeyRelease
    };
    let ev = gdk::Event::new(type_);
    let mut kev = ev
        .downcast::<gdk::EventKey>()
        .map_err(|_| "cast key event failed".to_string())?;
    {
        let k = std::convert::AsMut::<gdk::ffi::GdkEventKey>::as_mut(&mut kev);
        k.window = win.to_glib_full();
        k.send_event = 1;
        k.time = 0; // GDK_CURRENT_TIME
        k.state = 0;
        k.keyval = keyval;
        k.hardware_keycode = keycode as u16;
        k.group = 0;
        k.is_modifier = 0;
    }
    kev.put();
    Ok(())
}

/// 构造并投递一次指针移动事件
fn put_motion(win: &gdk::Window, x: f64, y: f64) -> ControlResult<()> {
    let ev = gdk::Event::new(gdk::EventType::MotionNotify);
    let mut mev = ev
        .downcast::<gdk::EventMotion>()
        .map_err(|_| "cast motion event failed".to_string())?;
    {
        let m = std::convert::AsMut::<gdk::ffi::GdkEventMotion>::as_mut(&mut mev);
        m.window = win.to_glib_full();
        m.send_event = 1;
        m.time = 0;
        m.x = x;
        m.y = y;
        m.x_root = x;
        m.y_root = y;
        m.axes = std::ptr::null_mut();
        m.state = 0;
        m.is_hint = 0;
        m.device = std::ptr::null_mut();
    }
    mev.put();
    Ok(())
}

/// 构造并投递一次鼠标按键事件
fn put_button(win: &gdk::Window, x: f64, y: f64, button: u32, press: bool) -> ControlResult<()> {
    let type_ = if press {
        gdk::EventType::ButtonPress
    } else {
        gdk::EventType::ButtonRelease
    };
    let ev = gdk::Event::new(type_);
    let mut bev = ev
        .downcast::<gdk::EventButton>()
        .map_err(|_| "cast button event failed".to_string())?;
    {
        let b = std::convert::AsMut::<gdk::ffi::GdkEventButton>::as_mut(&mut bev);
        b.window = win.to_glib_full();
        b.send_event = 1;
        b.time = 0;
        b.x = x;
        b.y = y;
        b.x_root = x;
        b.y_root = y;
        b.axes = std::ptr::null_mut();
        b.state = 0;
        b.button = button;
        b.device = std::ptr::null_mut();
    }
    bev.put();
    Ok(())
}

/// `char` → GDK keyval(GDK 的 `gdk_unicode_to_keyval`)
pub fn keyval_from_char(c: char) -> u32 {
    *gdk::keys::Key::from_unicode(c)
}
