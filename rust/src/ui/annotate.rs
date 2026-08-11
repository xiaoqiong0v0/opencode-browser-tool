//! 批注交互:Rust 侧状态机
//! 覆盖层 Webview 上报鼠标事件 → elementFromPoint 查询 → 更新高亮/批注标记
//! 覆盖层显示时拦截鼠标(批注模式);隐藏时页面正常交互
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

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
    pub fn toggle(app: &AppHandle, state: &State<UiState>) -> Result<bool, String> {
        let mut mode = state.annotate_mode.lock().unwrap();
        *mode = !*mode;
        let on = *mode;
        // 覆盖层显示状态:批注模式显示(拦截鼠标),否则隐藏(页面交互)
        let overlay = ui::overlay_webview(app).ok_or("overlay webview not ready")?;
        if on {
            overlay.show().map_err(|e| format!("overlay show failed: {e}"))?;
        } else {
            overlay.hide().map_err(|e| format!("overlay hide failed: {e}"))?;
            // 退出时清空高亮
            let _ = ui::eval_overlay(app, "window.__pwOverlay.redraw([], [])");
        }
        Ok(on)
    }

    /// 处理覆盖层上报的鼠标事件
    pub fn on_input(app: &AppHandle, state: &State<UiState>, ev: &Value) {
        let on = *state.annotate_mode.lock().unwrap();
        if !on {
            return;
        }
        let typ = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let x = ev.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let y = ev.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        // 临时诊断
        if std::env::var("PW_ANNOTATE_DEBUG").is_ok() {
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
                                "window.__pwOverlay.redraw([{{x:{},y:{},w:{},h:{},color:{}}}], window.__pwOverlay._marks || [])",
                                rect.0, rect.1, rect.2, rect.3, HOVER_COLOR
                            ),
                        );
                    }
                    _ => {
                        let _ = ui::eval_overlay(
                            app,
                            "window.__pwOverlay.redraw([], window.__pwOverlay._marks || [])",
                        );
                    }
                }
            }
            "click" => {
                // 添加批注
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
                        let mut records = state.records.lock().unwrap();
                        let index = records.len() as u32 + 1;
                        println!("[annotate] #{index} {selector} rect={rect:?}");
                        records.push(AnnotationRecord {
                            index,
                            selector,
                            rect,
                            note: String::new(),
                        });
                        // 推送面板刷新 + 覆盖层重绘标记
                        let list = records.clone();
                        drop(records);
                        if let Some(panel) = ui::panel_webview(app) {
                            let _ = panel.emit("records-changed", list);
                        }
                        let marks_js = Self::build_marks_js(app, state);
                        let _ = ui::eval_overlay(
                            app,
                            &format!(
                                "window.__pwOverlay._marks = {marks_js}; window.__pwOverlay.redraw([], {marks_js})"
                            ),
                        );
                    }
                    _ => println!("[annotate] click at ({x},{y}): no element"),
                }
            }
            "right" => {
                // 右键退出批注模式
                let _ = Self::toggle(app, state);
            }
            _ => {}
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
