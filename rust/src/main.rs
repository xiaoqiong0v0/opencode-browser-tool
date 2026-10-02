//! bt-shell 主入口:Tauri 应用(跨平台多 Webview)
//! 单窗口:页面 Webview + 覆盖层 Webview + 面板 Webview
//! 启动参数兼容旧协议:--browsers-path, --browser, --session-isolation, --port
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use bt_shell::control;
use bt_shell::ui::annotate::Annotator;
use bt_shell::ui::{self, AnnotationRecord};
use tauri::{Listener, Manager, State};

/// 解析命令行参数(兼容旧 Node 服务协议)
fn parse_args() -> (String, u16, Option<String>) {
    let args: Vec<String> = std::env::args().collect();
    let mut browsers_path = String::new();
    let mut port: u16 = 0;
    let mut user_data_dir: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--browsers-path" => {
                i += 1;
                if i < args.len() {
                    browsers_path = args[i].clone();
                }
            }
            "--port" => {
                i += 1;
                if i < args.len() {
                    port = args[i].parse().unwrap_or(0);
                }
            }
            "--user-data-dir" => {
                i += 1;
                if i < args.len() {
                    user_data_dir = Some(args[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }
    (browsers_path, port, user_data_dir)
}

/// 面板拉取批注记录
#[tauri::command]
fn panel_records(state: State<ui::UiState>) -> Vec<AnnotationRecord> {
    state.records.lock().unwrap().clone()
}

/// 更新批注说明(面板输入框 → Rust)
#[tauri::command]
fn panel_set_note(state: State<ui::UiState>, index: u32, note: String) -> Result<(), String> {
    let mut records = state.records.lock().unwrap();
    if let Some(r) = records.iter_mut().find(|r| r.index == index) {
        r.note = note;
        Ok(())
    } else {
        Err(format!("record #{index} not found"))
    }
}

/// 面板命令(面板 Webview 前端事件 → Rust)
#[tauri::command]
fn panel_cmd(
    app: tauri::AppHandle,
    state: State<ui::UiState>,
    cmd: String,
) -> Result<bool, String> {
    println!("[panel-cmd] {cmd}");
    match cmd.as_str() {
        "toggle-annotate" => Annotator::toggle(&app, &state),
        "toggle-shot" => Annotator::toggle_shot(&app, &state),
        "shot-now" => {
            // 立即截全屏并显示预览:进入截图模式,并取消批注模式(互斥)
            *state.shot_mode.lock().unwrap() = true;
            *state.annotate_mode.lock().unwrap() = false;
            *state.pending_click.lock().unwrap() = None;
            // 立即广播模式状态,工具栏批注/截图按钮激活态同步(不再等截图关闭才变色)
            Annotator::emit_mode_state(&app, &state);
            // 关闭面板(如有)
            if *state.panel_open.lock().unwrap() {
                ui::collapse_panel(&app);
            }
            ui::show_overlay(&app);
            let _ = ui::eval_overlay(&app, "window.__btOverlay.setMode('shot')");
            let _ = ui::eval_overlay(&app, "window.__btOverlay.hideMask()");
            let app2 = app.clone();
            std::thread::spawn(move || {
                match crate::control::screenshot::screenshot(&app2) {
                    Ok(img) => {
                        // 取 viewport 尺寸作为截图区域记录
                        let (w, h) = ui::eval_page(&app2, "JSON.stringify({w: innerWidth, h: innerHeight})")
                            .ok()
                            .and_then(|s| {
                                let v: serde_json::Value = serde_json::from_str(&s).ok()?;
                                Some((
                                    v["w"].as_i64().unwrap_or(0) as i32,
                                    v["h"].as_i64().unwrap_or(0) as i32,
                                ))
                            })
                            .unwrap_or((0, 0));
                        Annotator::show_shot_preview(&app2, img, (0, 0, w, h));
                    }
                    Err(e) => {
                        let msg = format!("截图失败:{e}");
                        let _ = ui::eval_overlay(&app2, &format!("window.__btOverlay.notify({msg:?},'err')"));
                    }
                }
            });
            Ok(true)
        }
        "send-all" => {
            // 发送所有记录:快照入队,插件端轮询 consume
            let count = ui::send_all_records(&app);
            Ok(count > 0)
        }
        _ => Err(format!("unknown panel cmd: {cmd}")),
    }
}

/// 面板获取当前模式状态(批注/截图开关),用于按钮激活态恢复
#[tauri::command]
fn panel_mode(state: State<ui::UiState>) -> serde_json::Value {
    let annotate = *state.annotate_mode.lock().unwrap();
    let shot = *state.shot_mode.lock().unwrap();
    serde_json::json!({ "annotate": annotate, "shot": shot })
}

/// 面板获取 HTTP 服务端口(用于 fetch /api/* 调用设备切换/开发者工具)
#[tauri::command]
fn panel_service_port(state: State<ui::UiState>) -> u16 {
    *state.service_port.lock().unwrap()
}

/// 设置主题模式(auto/light/dark),广播给所有 webview 生效
#[tauri::command]
fn panel_set_theme(app: tauri::AppHandle, state: State<'_, ui::UiState>, theme: String) -> Result<(), String> {
    if !matches!(theme.as_str(), "auto" | "light" | "dark") {
        return Err(format!("invalid theme: {theme} (auto/light/dark)"));
    }
    *state.theme.lock().unwrap() = theme.clone();
    // 页面 webview 背景跟随主题(用 webview 背景色,不注入修改网页)
    let bg = match theme.as_str() {
        "light" => Some(tauri::window::Color(245, 245, 245, 255)),
        _ => Some(tauri::window::Color(24, 24, 24, 255)),
    };
    for i in 1..=ui::MAX_TABS {
        if let Some(w) = app.get_webview(&format!("page-{i}")) {
            let _ = w.set_background_color(bg);
        }
    }
    use tauri::Emitter;
    let _ = app.emit("theme-changed", theme);
    Ok(())
}

// ---- 标签状态统一管理:后台轮询检测变化,有变更才广播事件,前端只响应事件不轮询 ----

/// 页面图标地址 JS:优先 <link rel~="icon">,回退 origin/favicon.ico;无则返回空串
/// (tauri:/about: 等内部页面不生成图标地址)
const ICON_JS: &str = r#"(function(){try{var l=document.querySelector('link[rel~="icon"]');if(l&&l.href)return l.href;var p=location.protocol;var o=location.origin;if(o&&o!=='null'&&p!=='tauri:'&&p!=='about:')return o+'/favicon.ico';}catch(e){}return '';})()"#;

/// 后台轮询:刷新激活标签的标题/URL/图标(页面导航后变化),返回是否有变化
fn poll_tab_title(app: &tauri::AppHandle) -> bool {
    let state = app.state::<ui::UiState>();
    let active = *state.active_tab.lock().unwrap();
    // 当前激活标签的 title/url/icon 快照
    let before = state
        .tabs
        .lock()
        .unwrap()
        .iter()
        .find(|t| t.id == active)
        .map(|t| (t.title.clone(), t.url.clone(), t.icon.clone()));
    // 从页面 webview 读取最新状态(阻塞线程 eval)
    let handle = app.clone();
    let page = tokio::task::block_in_place(move || control::page_state(&handle)).ok();
    // 图标地址:同一阻塞上下文再 eval 一次(空串表示无图标)
    let icon_handle = app.clone();
    let icon = tokio::task::block_in_place(move || control::eval(&icon_handle, ICON_JS))
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let mut changed = false;
    if let Some(page) = page {
        let url = page["url"].as_str().unwrap_or("").to_string();
        let title = page["title"].as_str().unwrap_or("").to_string();
        let state = app.state::<ui::UiState>();
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(t) = tabs.iter_mut().find(|t| t.id == active) {
            if !url.is_empty() {
                t.url = url.clone();
            }
            if !title.is_empty() {
                t.title = title.clone();
            }
            // 仅在拿到非空图标时更新,避免页面短暂无图标时闪烁
            if !icon.is_empty() {
                t.icon = icon.clone();
            }
            changed = before != Some((t.title.clone(), t.url.clone(), t.icon.clone()));
        }
    }
    changed
}

// ---- 工具栏命令:标签(伪多标签) + 地址栏导航 + 面板浮层开关 ----
// 注意:tauri command 默认在主线程执行,而 control::eval 的回调也需要主线程事件循环,
// 直接调用会死锁卡死整个窗口。因此涉及 eval 的操作一律 async + block_in_place 放到阻塞线程执行。

/// 在阻塞线程执行 UI 控制操作(避免 command 主线程与 eval 回调互等死锁)
fn ui_block<T, F>(app: &tauri::AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&tauri::AppHandle) -> Result<T, String> + Send + 'static,
{
    let handle = app.clone();
    tokio::task::block_in_place(move || f(&handle))
}

/// 工具栏状态:标签列表 + 激活标签 + 面板开关(同时刷新激活标签的 URL/标题)
#[tauri::command]
async fn toolbar_state(app: tauri::AppHandle, state: State<'_, ui::UiState>) -> Result<serde_json::Value, String> {
    let active = *state.active_tab.lock().unwrap();
    // 从页面 webview 刷新激活标签的 url/title(阻塞线程执行,避免死锁)
    if let Ok(page) = ui_block(&app, |h| control::page_state(h)) {
        let url = page["url"].as_str().unwrap_or("").to_string();
        let title = page["title"].as_str().unwrap_or("").to_string();
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(t) = tabs.iter_mut().find(|t| t.id == active) {
            if !url.is_empty() { t.url = url; }
            if !title.is_empty() { t.title = title; }
        }
    }
    let tabs = state.tabs.lock().unwrap().clone();
    let panel_open = *state.panel_open.lock().unwrap();
    Ok(serde_json::json!({ "tabs": tabs, "active": active, "panel_open": panel_open }))
}

/// 新建标签页(工具栏"+"按钮 → 新标签页 / 指定地址)
#[tauri::command]
async fn toolbar_new_tab(app: tauri::AppHandle, state: State<'_, ui::UiState>, url: Option<String>) -> Result<(), String> {
    let url = url.unwrap_or_else(|| ui::NEWTAB_URL.to_string());
    let _ = state;
    ui::open_new_tab(&app, &url)
}

/// 切换标签:隐藏当前 webview,显示目标 webview(不重新加载,保留页面状态)
#[tauri::command]
async fn toolbar_switch_tab(app: tauri::AppHandle, state: State<'_, ui::UiState>, id: u32) -> Result<(), String> {
    let _ = state;
    ui::switch_tab(&app, id)
}

/// 关闭标签:销毁其 webview,切换到邻近标签(关闭最后一个则自动新建空白标签)
#[tauri::command]
async fn toolbar_close_tab(app: tauri::AppHandle, state: State<'_, ui::UiState>, id: u32) -> Result<(), String> {
    let _ = state;
    ui::close_tab(&app, id)
}

/// 地址栏导航:更新激活标签 URL 并导航
#[tauri::command]
async fn toolbar_navigate(app: tauri::AppHandle, state: State<'_, ui::UiState>, url: String) -> Result<(), String> {
    if url.is_empty() {
        return Err("url is required".into());
    }
    // 裸域名自动补 http://
    let url = ui::normalize_url(&url);
    let active = *state.active_tab.lock().unwrap();
    let mut tabs = state.tabs.lock().unwrap();
    if let Some(t) = tabs.iter_mut().find(|t| t.id == active) {
        t.url = url.clone();
    }
    drop(tabs);
    ui_block(&app, move |h| control::navigate(h, &url))?;
    ui::emit_tabs_changed(&app);
    Ok(())
}

/// 切换面板浮层显示,返回新状态
#[tauri::command]
fn toolbar_toggle_panel(app: tauri::AppHandle, state: State<'_, ui::UiState>) -> Result<bool, String>
{
    let open = state.panel_open.lock().unwrap();
    let now = !*open;
    drop(open);
    if now {
        // 打开面板时退出批注/截图模式(模式与遮罩互斥,避免覆盖层遮挡)
        let annotate = *state.annotate_mode.lock().unwrap();
        if annotate {
            let _ = Annotator::toggle(&app, &state);
        }
        let shot = *state.shot_mode.lock().unwrap();
        if shot {
            let _ = Annotator::toggle_shot(&app, &state);
        }
        // 显示透明覆盖层 + 面板滑入动画(遮罩盖页面区,点击遮罩关闭面板)
        ui::show_overlay(&app);
        ui::open_panel(&app);
    } else {
        ui::close_panel(&app);
    }
    Ok(now)
}

/// 确保面板关闭(非 toggle,供面板内操作执行后统一收起,避免二次 toggle 重新打开)
#[tauri::command]
fn toolbar_close_panel(app: tauri::AppHandle, state: State<'_, ui::UiState>) -> Result<(), String> {
    let open = *state.panel_open.lock().unwrap();
    if !open {
        return Ok(());
    }
    ui::close_panel(&app);
    Ok(())
}
/// 页面后退(返回历史上一页,通过 eval history.back,阻塞线程执行)
#[tauri::command]
async fn toolbar_go_back(app: tauri::AppHandle) -> Result<(), String> {
    ui_block(&app, |h| control::eval(h, "history.back()").map(|_| ()))
}

/// 页面前进(通过 eval history.forward)
#[tauri::command]
async fn toolbar_go_forward(app: tauri::AppHandle) -> Result<(), String> {
    ui_block(&app, |h| control::eval(h, "history.forward()").map(|_| ()))
}

/// 页面刷新(通过 eval location.reload)
#[tauri::command]
async fn toolbar_reload(app: tauri::AppHandle) -> Result<(), String> {
    ui_block(&app, |h| control::eval(h, "location.reload()").map(|_| ()))
}

fn main() {
    // 显式声明 DPI 感知(否则窗口被系统虚拟化缩放,与 WebView2 真实 DPI 不一致导致布局错位)
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let (browsers_path, port, user_data_dir) = parse_args();

    tauri::Builder::default()
        .manage(ui::UiState::new())
        .invoke_handler(tauri::generate_handler![
            panel_records,
            panel_set_note,
            panel_cmd,
            panel_mode,
            panel_service_port,
            panel_set_theme,
            toolbar_state,
            toolbar_new_tab,
            toolbar_switch_tab,
            toolbar_close_tab,
            toolbar_navigate,
            toolbar_toggle_panel,
            toolbar_close_panel,
            toolbar_go_back,
            toolbar_go_forward,
            toolbar_reload
        ])
        .setup(move |app| {
            // 设置用户数据目录(多用户配置隔离),再创建 Webview(create_ui 据此设置 data_directory)
            {
                let state = app.state::<ui::UiState>();
                *state.user_data_dir.lock().unwrap() = user_data_dir.clone();
            }
            // 创建三 Webview 布局
            ui::create_ui(app.handle())?;

            // 记录服务端口供面板查询
            {
                let state = app.state::<ui::UiState>();
                *state.service_port.lock().unwrap() = port;
            }

            // 启动 HTTP 服务(端口由 --port 指定,默认 0 随机)
            let http_app = Arc::new(bt_shell::service::App::new(
                app.handle().clone(),
                browsers_path,
            ));
            let http_app2 = http_app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = bt_shell::http::serve(http_app2, port).await;
            });

            // 事件监听:覆盖层鼠标事件 → 批注状态机
            {
                let handle = app.handle().clone();
                app.listen_any("overlay-input", move |event| {
                    let payload = event.payload();
                    if let Ok(ev) = serde_json::from_str::<serde_json::Value>(payload) {
                        let state = handle.state::<ui::UiState>();
                        Annotator::on_input(&handle, &state, &ev);
                    }
                });
            }

            // 标签状态统一管理:后台每秒轮询检测标题/URL 变化,有变更才广播 tabs-changed
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let changed = tokio::task::block_in_place(|| poll_tab_title(&handle));
                        if changed {
                            ui::emit_tabs_changed(&handle);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // TODO: 清理浏览器状态
            }
        });
}
