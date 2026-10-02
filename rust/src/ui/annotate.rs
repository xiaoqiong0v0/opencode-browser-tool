//! 批注/截图交互:Rust 侧状态机
//! 覆盖层 Webview 上报鼠标事件 → 批注模式查询元素弹输入框,截图模式截取区域/全屏
//! 覆盖层显示时拦截鼠标;隐藏时页面正常交互
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ui::{self, AnnotationRecord, PendingShot, RECORD_ANNOTATE, RECORD_SCREENSHOT, UiState};

/// 悬停高亮色(0xRRGGBB)
const HOVER_COLOR: u32 = 0xFFC107;

/// 批注/截图状态机
pub struct Annotator {}

impl Annotator {
    pub fn new() -> Self {
        Self {}
    }

    /// 切换批注模式(覆盖层显示/隐藏)
    /// 开:显示覆盖层拦截鼠标,同时关闭右侧功能面板(批注需全页面交互),并取消截图模式
    /// 关:隐藏覆盖层,页面恢复交互
    pub fn toggle(app: &AppHandle, state: &State<UiState>) -> Result<bool, String> {
        let mut mode = state.annotate_mode.lock().unwrap();
        *mode = !*mode;
        let on = *mode;
        // 提前释放锁(emit_mode_state 需重新加锁,std Mutex 不可重入)
        drop(mode);
        let overlay = ui::overlay_webview(app).ok_or("overlay webview not ready")?;
        let _ = ui::eval_overlay(app, "window.__btOverlay.hideMask()");
        if on {
            // 互斥:开启批注时取消截图模式
            *state.shot_mode.lock().unwrap() = false;
            *state.pending_shot.lock().unwrap() = None;
            let _ = ui::eval_overlay(app, "window.__btOverlay.hideShotPreview()");
            // 批注模式需要全页面交互,自动关闭右侧面板
            Self::close_panel_if_open(app, state);
        } else {
            // 退出时清空高亮、批注弹框与待确认批注
            let _ = ui::eval_overlay(app, "window.__btOverlay.redraw([], [])");
            let _ = ui::eval_overlay(app, "window.__btOverlay.hideNotePop()");
            *state.pending_click.lock().unwrap() = None;
        }
        let _ = overlay;
        // 按当前模式状态同步覆盖层显示
        Self::sync_overlay_display(app, state);
        // 广播模式状态,面板/工具栏按钮激活态同步
        Self::emit_mode_state(app, state);
        Ok(on)
    }

    /// 切换截图模式
    /// 开:显示覆盖层拦截鼠标,双击截全屏 / 拖动框选截区域,并取消批注模式
    /// 关:隐藏覆盖层,页面恢复交互
    pub fn toggle_shot(app: &AppHandle, state: &State<UiState>) -> Result<bool, String> {
        let mut shot = state.shot_mode.lock().unwrap();
        *shot = !*shot;
        let on = *shot;
        // 提前释放锁(emit_mode_state 需重新加锁,std Mutex 不可重入)
        drop(shot);
        let _ = ui::eval_overlay(app, "window.__btOverlay.hideMask()");
        if on {
            // 互斥:开启截图时取消批注模式
            *state.annotate_mode.lock().unwrap() = false;
            *state.pending_click.lock().unwrap() = None;
            let _ = ui::eval_overlay(app, "window.__btOverlay.hideNotePop()");
            Self::close_panel_if_open(app, state);
        } else {
            *state.pending_shot.lock().unwrap() = None;
            let _ = ui::eval_overlay(app, "window.__btOverlay.hideShotPreview()");
        }
        // 按当前模式状态同步覆盖层显示
        Self::sync_overlay_display(app, state);
        // 广播模式状态,面板/工具栏按钮激活态同步
        Self::emit_mode_state(app, state);
        Ok(on)
    }

    /// 按当前模式状态同步覆盖层显示:
    /// 批注/截图可共存,优先显示最近激活的模式;两者都关时隐藏覆盖层
    fn sync_overlay_display(app: &AppHandle, state: &State<UiState>) {
        let annotate = *state.annotate_mode.lock().unwrap();
        let shot = *state.shot_mode.lock().unwrap();
        if annotate {
            let _ = ui::eval_overlay(app, "window.__btOverlay.setMode('annotate')");
            ui::show_overlay(app);
        } else if shot {
            let _ = ui::eval_overlay(app, "window.__btOverlay.setMode('shot')");
            ui::show_overlay(app);
        } else {
            let _ = ui::eval_overlay(app, "window.__btOverlay.setMode('none')");
            if let Some(overlay) = ui::overlay_webview(app) {
                let _ = overlay.hide();
            }
        }
    }

    /// 完成单次截图(立即截图后):退出截图模式并隐藏覆盖层(不恢复批注,已互斥取消)
    fn complete_shot(app: &AppHandle, state: &State<UiState>) {
        *state.shot_mode.lock().unwrap() = false;
        *state.pending_shot.lock().unwrap() = None;
        let _ = ui::eval_overlay(app, "window.__btOverlay.hideShotPreview()");
        let _ = ui::eval_overlay(app, "window.__btOverlay.setMode('none')");
        if let Some(overlay) = ui::overlay_webview(app) {
            let _ = overlay.hide();
        }
        Self::emit_mode_state(app, state);
    }

    /// 广播批注/截图模式状态,面板与工具栏按钮激活态同步(全局广播,各 webview 各自 listen)
    pub fn emit_mode_state(app: &AppHandle, state: &State<UiState>) {
        let annotate = *state.annotate_mode.lock().unwrap();
        let shot = *state.shot_mode.lock().unwrap();
        let _ = app.emit("annotate-state", json!({ "annotate": annotate, "shot": shot }));
    }

    /// 处理覆盖层上报的鼠标事件
    pub fn on_input(app: &AppHandle, state: &State<UiState>, ev: &Value) {
        let on = *state.annotate_mode.lock().unwrap();
        let shot_on = *state.shot_mode.lock().unwrap();
        let typ = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
        // 截图模式:只处理截图事件(批注事件忽略)
        if shot_on {
            Self::on_shot_input(app, state, typ, ev);
            return;
        }
        if !on {
            // 非批注模式:面板遮罩点击 → 关闭面板
            if typ == "mask-click" {
                Self::close_panel(app, state);
            }
            return;
        }
        let x = ev.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let y = ev.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
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
                // 弹框确定:有说明文字才保存批注入列,无则丢弃
                let text = ev.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let pending = state.pending_click.lock().unwrap().take();
                if let Some(p) = pending {
                    if !text.trim().is_empty() {
                        Self::push_record(app, state, RECORD_ANNOTATE, &p.selector, p.rect, &text, None);
                    }
                }
            }
            "note-send" => {
                // 弹框发送:有说明先保存当前批注,再发送所有记录,随后退出批注模式
                let text = ev.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let pending = state.pending_click.lock().unwrap().take();
                if let Some(p) = pending {
                    if !text.trim().is_empty() {
                        Self::push_record(app, state, RECORD_ANNOTATE, &p.selector, p.rect, &text, None);
                    }
                }
                let count = ui::send_all_records(app);
                if let Some(panel) = ui::panel_webview(app) {
                    let _ = panel.emit("records-sent", count);
                }
                // 发送后退出批注模式(取消工具激活)
                if *state.annotate_mode.lock().unwrap() {
                    let _ = Self::toggle(app, state);
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

    /// 处理截图模式事件(在后台线程执行截图,避免阻塞事件线程)
    fn on_shot_input(app: &AppHandle, state: &State<UiState>, typ: &str, ev: &Value) {
        match typ {
            "shot-full" => {
                // 双击截全屏(viewport 尺寸由覆盖层上报)
                let w = ev.get("w").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let h = ev.get("h").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let app2 = app.clone();
                std::thread::spawn(move || {
                    match crate::control::screenshot::screenshot(&app2) {
                        Ok(img) => {
                            let rect = (0, 0, w, h);
                            Self::show_shot_preview(&app2, img, rect);
                        }
                        Err(e) => {
                            let msg = format!("截图失败:{e}");
                            let _ = ui::eval_overlay(&app2, &format!("window.__btOverlay.notify({msg:?},'err')"));
                        }
                    }
                });
            }
            "shot-select" => {
                // 拖动框选结束 → 截取选区区域
                let x = ev.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let y = ev.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let w = ev.get("w").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let h = ev.get("h").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                if w < 5 || h < 5 {
                    return;
                }
                let app2 = app.clone();
                std::thread::spawn(move || {
                    match crate::control::screenshot::screenshot_clip(&app2, x, y, w, h) {
                        Ok(img) => {
                            Self::show_shot_preview(&app2, img, (x, y, w, h));
                        }
                        Err(e) => {
                            let msg = format!("截图失败:{e}");
                            let _ = ui::eval_overlay(&app2, &format!("window.__btOverlay.notify({msg:?},'err')"));
                        }
                    }
                });
            }
            "shot-cancel" => {
                // 取消本次截图,完成单次截图(恢复批注/隐藏覆盖层)
                Self::complete_shot(app, state);
            }
            "shot-save" => {
                // 保存截图记录:有说明文字才保存,无则丢弃,完成后单次截图
                let text = ev.get("note").and_then(|t| t.as_str()).unwrap_or("").to_string();
                if !text.trim().is_empty() {
                    // 前端裁剪后的 base64(按选区裁剪),空则用整图
                    let crop = ev.get("image").and_then(|i| i.as_str()).map(|s| s.to_string());
                    let pending = state.pending_shot.lock().unwrap().take();
                    if let Some(p) = pending {
                        let image = crop.filter(|s| !s.is_empty()).unwrap_or(p.image);
                        Self::push_record(app, state, RECORD_SCREENSHOT, "", p.rect, &text, Some(image));
                    }
                } else {
                    *state.pending_shot.lock().unwrap() = None;
                }
                Self::complete_shot(app, state);
            }
            "shot-send" => {
                // 发送:有说明先保存当前截图,再发送所有记录,完成后单次截图
                let text = ev.get("note").and_then(|t| t.as_str()).unwrap_or("").to_string();
                if !text.trim().is_empty() {
                    let crop = ev.get("image").and_then(|i| i.as_str()).map(|s| s.to_string());
                    let pending = state.pending_shot.lock().unwrap().take();
                    if let Some(p) = pending {
                        let image = crop.filter(|s| !s.is_empty()).unwrap_or(p.image);
                        Self::push_record(app, state, RECORD_SCREENSHOT, "", p.rect, &text, Some(image));
                    }
                } else {
                    *state.pending_shot.lock().unwrap() = None;
                }
                let count = ui::send_all_records(app);
                Self::complete_shot(app, state);
                if let Some(panel) = ui::panel_webview(app) {
                    let _ = panel.emit("records-sent", count);
                }
            }
            "right" => {
                // 右键退出截图模式
                let _ = Self::toggle_shot(app, state);
            }
            _ => {}
        }
    }

    /// 截图完成:暂存待确认截图并显示预览(面板"立即截图"与截图模式共用)
    pub fn show_shot_preview(app: &AppHandle, image: String, rect: (i32, i32, i32, i32)) {
        let state = app.state::<UiState>();
        *state.pending_shot.lock().unwrap() = Some(PendingShot { image: image.clone(), rect });
        let _ = ui::eval_overlay(
            app,
            &format!(
                "window.__btOverlay.showShotPreview({image:?}, {x}, {y}, {w}, {h})",
                image = format!("data:image/png;base64,{image}"),
                x = rect.0,
                y = rect.1,
                w = rect.2,
                h = rect.3
            ),
        );
    }

    /// 关闭面板(遮罩点击触发):完整关闭(退出模式 + 清理弹窗),并广播状态同步工具栏按钮
    pub fn close_panel(app: &AppHandle, state: &State<UiState>) {
        let _ = state;
        ui::close_panel(app);
    }

    /// 面板打开时收起(批注/截图模式需要全页面交互;仅收起不退出刚开启的模式)
    fn close_panel_if_open(app: &AppHandle, state: &State<UiState>) {
        let open = *state.panel_open.lock().unwrap();
        if open {
            ui::collapse_panel(app);
        }
    }

    /// 记录入列(批注/截图共用):附带当前页面地址,推送面板;页面不再绘制标点(地址/滚动定位不准)
    fn push_record(
        app: &AppHandle,
        state: &State<UiState>,
        typ: &str,
        selector: &str,
        rect: (i32, i32, i32, i32),
        note: &str,
        image: Option<String>,
    ) {
        let url = ui::active_tab_url(app);
        let mut records = state.records.lock().unwrap();
        let index = records.len() as u32 + 1;
        records.push(AnnotationRecord {
            index,
            typ: typ.to_string(),
            url,
            selector: selector.to_string(),
            rect,
            note: note.to_string(),
            image,
            sent: false,
        });
        let list = records.clone();
        drop(records);
        if let Some(panel) = ui::panel_webview(app) {
            let _ = panel.emit("records-changed", list);
        }
    }
}

/// 注册批注相关事件监听
pub fn register(_app: &AppHandle, state: tauri::State<'_, UiState>) {
    let _ = state;
    let _ = json!({});
}
