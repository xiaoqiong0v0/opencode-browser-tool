//! 批注交互:Rust 侧状态机
//! 覆盖层 Webview 上报鼠标事件 → elementFromPoint 查询 → 更新高亮/批注标记
//! 覆盖层显示时拦截鼠标(批注模式);隐藏时页面正常交互
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ui::{self, AnnotationRecord, UiState};

/// 悬停高亮色(0xRRGGBB)
const HOVER_COLOR: u32 = 0xFFC107;
/// 批注标记色
const MARK_COLOR: u32 = 0x4CAF50;

/// 批注状态机
pub struct Annotator {
    /// 当前悬停高亮
    hover: Option<(i32, i32, i32, i32)>,
}

impl Annotator {
    pub fn new() -> Self {
        Self { hover: None }
    }

    /// 切换批注模式(覆盖层显示/隐藏)
    /// 开:显示覆盖层拦截鼠标,同时关闭右侧功能面板(批注需全页面交互)
    /// 关:隐藏覆盖层,页面恢复交互
    pub fn toggle(app: &AppHandle, state: &State<UiState>) -> Result<bool, String> {
        let mut mode = state.annotate_mode.lock().unwrap();
        *mode = !*mode;
        let on = *mode;
        // 覆盖层显示状态:批注模式显示(拦截鼠标),否则隐藏(页面交互)
        let overlay = ui::overlay_webview(app).ok_or("overlay webview not ready")?;
        if on {
            // 批注模式:隐藏面板遮罩(避免干扰),显示覆盖层拦截鼠标
            let _ = ui::eval_overlay(app, "window.__btOverlay.hideMask()");
            overlay.show().map_err(|e| format!("overlay show failed: {e}"))?;
            // 批注模式需要全页面交互,自动关闭右侧面板
            let mut panel_open = state.panel_open.lock().unwrap();
            if *panel_open {
                *panel_open = false;
                drop(panel_open);
                if let Some(win) = app.get_window("main") {
                    let size = win.inner_size().unwrap_or_default();
                    let scale = win.scale_factor().unwrap_or(1.0);
                    let _ = ui::apply_layout(app, size, scale);
                }
            }
        } else {
            overlay.hide().map_err(|e| format!("overlay hide failed: {e}"))?;
            // 退出时清空高亮与待确认批注
            let _ = ui::eval_overlay(app, "window.__btOverlay.redraw([], [])");
            *state.pending_click.lock().unwrap() = None;
        }
        Ok(on)
    }

    /// 处理覆盖层上报的鼠标事件
    pub fn on_input(app: &AppHandle, state: &State<UiState>, ev: &Value) {
        let on = *state.annotate_mode.lock().unwrap();
        let typ = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if !on {
            // 非批注模式:面板遮罩点击 → 关闭面板
            if typ == "mask-click" {
                Self::close_panel(app, state);
            }
            return;
        }
        let x = ev.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let y = ev.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        // 临时诊断
        if std::env::var("bt_ANNOTATE_DEBUG").is_ok() {
            println!("[annotate] input {typ} ({x},{y})");
            match crate::control::query_element(app, x, y) {
                Ok(v) => println!("[annotate] query: {v}"),
                Err(e) => println!("[annotate] query err: {e}"),
            }
        }
        match typ {
            "move" => {
                // 悬停高亮
                match crate::control::query_element(app, x, y) {
                    Ok(v) if !v.is_null() => {
                        let r = &v["rect"];
                        let rect = (
                            r["x"].as_f64().unwrap_or(0.0) as i32,
                            r["y"].as_f64().unwrap_or(0.0) as i32,
                            r["w"].as_f64().unwrap_or(0.0) as i32,
                            r["h"].as_f64().unwrap_or(0.0) as i32,
                        );
                        let _ = ui::eval_overlay(
                            app,
                            &format!(
                                "window.__btOverlay.redraw([{{x:{},y:{},w:{},h:{},color:{}}}], window.__btOverlay._marks || [])",
                                rect.0, rect.1, rect.2, rect.3, HOVER_COLOR
                            ),
                        );
                    }
                    _ => {
                        let _ = ui::eval_overlay(
                            app,
                            "window.__btOverlay.redraw([], window.__btOverlay._marks || [])",
                        );
                    }
                }
            }
            "click" => {
                // 点击元素 → 暂存待确认批注 + 点击位置弹输入框
                match crate::control::query_element(app, x, y) {
                    Ok(v) if !v.is_null() => {
                        let selector = v["selector"].as_str().unwrap_or("").to_string();
                        let r = &v["rect"];
                        let rect = (
                            r["x"].as_f64().unwrap_or(0.0) as i32,
                            r["y"].as_f64().unwrap_or(0.0) as i32,
                            r["w"].as_f64().unwrap_or(0.0) as i32,
                            r["h"].as_f64().unwrap_or(0.0) as i32,
                        );
                        *state.pending_click.lock().unwrap() = Some(ui::PendingClick {
                            selector: selector.clone(),
                            rect,
                        });
                        // 在点击位置显示批注输入弹框(坐标直接传给覆盖层)
                        let _ = ui::eval_overlay(
                            app,
                            &format!(
                                "window.__btOverlay.showNoteInput({x},{y},{sel:?})",
                                x = x,
                                y = y,
                                sel = selector
                            ),
                        );
                    }
                    _ => println!("[annotate] click at ({x},{y}): no element"),
                }
            }
            "note-submit" => {
                // 弹框确定:读取待确认批注入列,刷新面板 + 画标记
                let text = ev.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let pending = state.pending_click.lock().unwrap().take();
                if let Some(p) = pending {
                    let mut records = state.records.lock().unwrap();
                    let index = records.len() as u32 + 1;
                    println!("[annotate] #{index} {sel} rect={rect:?} note={note}", sel = p.selector, rect = p.rect, note = text);
                    records.push(AnnotationRecord {
                        index,
                        selector: p.selector,
                        rect: p.rect,
                        note: text,
                    });
                    let list = records.clone();
                    drop(records);
                    if let Some(panel) = ui::panel_webview(app) {
                        let _ = panel.emit("records-changed", list);
                    }
                    let marks_js = Self::build_marks_js(app, state);
                    let _ = ui::eval_overlay(
                        app,
                        &format!(
                            "window.__btOverlay._marks = {marks_js}; window.__btOverlay.redraw([], {marks_js})"
                        ),
                    );
                }
            }
            "note-cancel" => {
                // 弹框取消:丢弃待确认批注
                *state.pending_click.lock().unwrap() = None;
            }
            "right" => {
                // 右键退出批注模式
                let _ = Self::toggle(app, state);
            }
            _ => {}
        }
    }

    /// 关闭面板(遮罩点击触发):隐藏遮罩与覆盖层,重排面板消失
    pub fn close_panel(app: &AppHandle, state: &State<UiState>) {
        let mut open = state.panel_open.lock().unwrap();
        *open = false;
        drop(open);
        let _ = ui::eval_overlay(app, "window.__btOverlay.hideMask()");
        if let Some(overlay) = ui::overlay_webview(app) {
            let _ = overlay.hide();
        }
        if let Some(win) = app.get_window("main") {
            let size = win.inner_size().unwrap_or_default();
            let scale = win.scale_factor().unwrap_or(1.0);
            let _ = ui::apply_layout(app, size, scale);
        }
    }

    /// 从状态生成批注标记 JS 数组
    fn build_marks_js(app: &AppHandle, state: &State<UiState>) -> String {
        let records = state.records.lock().unwrap();
        let items: Vec<String> = records
            .iter()
            .map(|r| {
                format!(
                    "{{x:{},y:{},w:24,h:24,index:{},color:{}}}",
                    r.rect.0,
                    r.rect.1.saturating_sub(24),
                    r.index,
                    MARK_COLOR
                )
            })
            .collect();
        let _ = app;
        format!("[{}]", items.join(","))
    }
}

/// 注册批注相关事件监听
pub fn register(app: &AppHandle, state: tauri::State<'_, UiState>) {
    let _ = state;
    let _ = json!({});
}
