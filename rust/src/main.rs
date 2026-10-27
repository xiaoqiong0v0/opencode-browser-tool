//! bt-shell 主入口:Tauri 应用(跨平台多 Webview)
//! 单窗口:页面 Webview + 覆盖层 Webview + 面板 Webview
//! 启动参数兼容旧协议:--browsers-path, --browser, --session-isolation, --port
//! 1.10.0 新增 --token:HTTP API 鉴权令牌(缺省读 BT_SHELL_TOKEN,再缺省随机生成)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// MSVC link.exe 会把"正在创建库 .lib/.exp"打到 stdout,cargo 以 linker_messages 警告呈现——非代码问题,屏蔽以保持 0 警告
#![allow(linker_messages)]

use std::sync::Arc;

use bt_shell::control;
use bt_shell::streams;
use bt_shell::ui::annotate::Annotator;
use bt_shell::ui::{self, AnnotationRecord};
use tauri::{Listener, Manager, State};

/// 令牌落盘文件名(位于 --user-data-dir 内,仅用于排查)
const TOKEN_FILE_NAME: &str = "bt-auth-token";

/// 解析命令行参数(兼容旧 Node 服务协议)
/// 返回 (browsers_path, port, user_data_dir, notify_url, title, token)
fn parse_args() -> (
    String,
    u16,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let args: Vec<String> = std::env::args().collect();
    let mut browsers_path = String::new();
    let mut port: u16 = 0;
    let mut user_data_dir: Option<String> = None;
    // 通知地址:环境变量兜底(附着模式下插件通过 /api/notify-url 注册)
    let mut notify_url: Option<String> = std::env::var("BT_SHELL_NOTIFY_URL").ok();
    // 窗口标题:插件传 --title "bt-shell · <会话标识>" 便于任务栏分辨多个会话实例
    let mut title: Option<String> = None;
    // HTTP API 鉴权令牌:显式 --token 优先(缺省时由 resolve_auth_token 依次回落环境变量/随机)
    let mut token: Option<String> = None;
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
            "--notify-url" => {
                i += 1;
                if i < args.len() {
                    notify_url = Some(args[i].clone());
                }
            }
            "--title" => {
                i += 1;
                if i < args.len() {
                    title = Some(args[i].clone());
                }
            }
            "--token" => {
                i += 1;
                if i < args.len() {
                    token = Some(args[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }
    (browsers_path, port, user_data_dir, notify_url, title, token)
}

/// 解析事件流配置(1.12.0):env 为底,命令行 `--console-buffer-mb/--network-buffer-mb/
/// --network-burst-ms/--console-inject` 覆盖。命令行与 env 都由插件启动时传入(契约:
/// consoleBufferBytes / networkBufferBytes / networkBurstWindowMs / consoleInject)。
fn streams_config_from_args() -> streams::StreamsConfig {
    let mut cfg = streams::config_from_env();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let val = args.get(i + 1).map(String::as_str);
        match args[i].as_str() {
            "--console-buffer-mb" => {
                if let Some(n) = val.and_then(|s| s.trim().parse::<u64>().ok()) {
                    cfg.console_bytes = ((n as usize).saturating_mul(1024 * 1024)).max(64 * 1024);
                }
                i += 1;
            }
            "--network-buffer-mb" => {
                if let Some(n) = val.and_then(|s| s.trim().parse::<u64>().ok()) {
                    cfg.network_bytes = ((n as usize).saturating_mul(1024 * 1024)).max(64 * 1024);
                }
                i += 1;
            }
            "--network-burst-ms" => {
                if let Some(n) = val.and_then(|s| s.trim().parse::<u64>().ok()) {
                    cfg.burst_window_ms = n;
                }
                i += 1;
            }
            "--console-inject" => {
                if let Some(v) = val {
                    cfg.console_inject = matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "on" | "yes"
                    );
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    cfg
}

/// 解析鉴权令牌,返回 (令牌, 来源标识)
/// 优先级:命令行 --token → 环境变量 BT_SHELL_TOKEN → 随机生成(16 字节 → 32 hex)
fn resolve_auth_token(cli_token: Option<String>) -> (String, &'static str) {
    if let Some(t) = cli_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        return (t.to_string(), "cli");
    }
    if let Ok(env_token) = std::env::var("BT_SHELL_TOKEN") {
        let t = env_token.trim();
        if !t.is_empty() {
            return (t.to_string(), "env");
        }
    }
    (random_token(), "generated")
}

/// 生成 32 位十六进制随机令牌(16 字节)
///
/// 不引入新依赖:用 `RandomState`(进程启动时由 OS 熵初始化)对递增计数器做哈希,
/// 拼够 128 bit。仅用于本机回环 API 鉴权,不需要密码学级 CSPRNG 实现。
fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let state = RandomState::new();
    let mut out = String::with_capacity(32);
    let mut counter: u8 = 0;
    while out.len() < 32 {
        let mut hasher = state.build_hasher();
        hasher.write_u8(counter);
        counter = counter.wrapping_add(1);
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out.truncate(32);
    out
}

/// 日志里的令牌前缀:只留前 4 位;令牌本身不超过 4 位时完全打码
fn token_prefix(token: &str) -> String {
    if token.chars().count() <= 4 {
        "****".to_string()
    } else {
        token.chars().take(4).collect()
    }
}

/// 把令牌写入 `<user-data-dir>/bt-auth-token`(供排查;Unix 下 chmod 600),失败只记日志
fn persist_auth_token(user_data_dir: Option<&str>, token: &str) {
    let Some(dir) = user_data_dir.map(str::trim).filter(|d| !d.is_empty()) else {
        eprintln!("[auth] user-data-dir not set; token file not written");
        return;
    };
    let dir_path = std::path::Path::new(dir);
    if let Err(e) = std::fs::create_dir_all(dir_path) {
        eprintln!("[auth] create user-data-dir failed: {e}");
        return;
    }
    let path = dir_path.join(TOKEN_FILE_NAME);
    if let Err(e) = std::fs::write(&path, token) {
        eprintln!("[auth] write token file failed: {e}");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 失败不阻断(部分文件系统不支持权限位)
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    eprintln!("[auth] token file written: {}", path.display());
}

/// 面板拉取批注记录
#[tauri::command]
fn panel_records(state: State<ui::UiState>) -> Vec<AnnotationRecord> {
    // 面板列表只显示"未发送"记录(与参考实现一致:发送后列表清空;记录本身保留供工具读取)
    state
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|r| !r.sent)
        .cloned()
        .collect()
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
                        let (w, h) =
                            ui::eval_page(&app2, "JSON.stringify({w: innerWidth, h: innerHeight})")
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
                        let _ = ui::eval_overlay(
                            &app2,
                            &format!("window.__btOverlay.notify({msg:?},'err')"),
                        );
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

/// 面板读取 API 令牌所用的受管状态(实例创建期确定,运行期只读)
struct ApiToken(Arc<str>);

/// 面板获取 HTTP API 令牌(面板 fetch /api/* 时带 `x-bt-token`)
///
/// 面板 Webview 与 shell 同进程,通过 Tauri IPC 读取,不经网络;
/// 令牌只进入面板 JS 内存,不写入 DOM/HTML,也不写日志。
#[tauri::command]
fn api_token(state: State<'_, ApiToken>) -> String {
    state.0.to_string()
}

/// 设置主题模式(auto/light/dark),广播给所有 webview 生效
#[tauri::command]
fn panel_set_theme(
    app: tauri::AppHandle,
    state: State<'_, ui::UiState>,
    theme: String,
) -> Result<(), String> {
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

// ---- 标签状态统一管理:事件驱动(on_navigation/on_page_load + 页面 /api/page-event 上报),
//      有变更才广播 tabs-changed;前端只响应事件,无轮询 ----

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
async fn toolbar_state(
    app: tauri::AppHandle,
    state: State<'_, ui::UiState>,
) -> Result<serde_json::Value, String> {
    let active = *state.active_tab.lock().unwrap();
    // 从页面 webview 读一次状态(非轮询,仅工具栏拉取时);走同一守卫,错误页不覆盖地址栏
    if let Some(label) = ui::active_page_webview(&app).map(|w| w.label().to_string()) {
        if let Ok(page) = ui_block(&app, |h| control::page_state(h)) {
            let url = page["url"].as_str().unwrap_or("");
            let title = page["title"].as_str().unwrap_or("");
            ui::apply_page_state(
                &app,
                &label,
                if url.is_empty() { None } else { Some(url) },
                if title.is_empty() { None } else { Some(title) },
                None,
            );
        }
    }
    let tabs = state.tabs.lock().unwrap().clone();
    let panel_open = *state.panel_open.lock().unwrap();
    Ok(serde_json::json!({ "tabs": tabs, "active": active, "panel_open": panel_open }))
}

/// 新建标签页(工具栏"+"按钮 → 新标签页 / 指定地址)
#[tauri::command]
async fn toolbar_new_tab(
    app: tauri::AppHandle,
    state: State<'_, ui::UiState>,
    url: Option<String>,
) -> Result<(), String> {
    let url = url.unwrap_or_else(|| ui::NEWTAB_URL.to_string());
    let _ = state;
    ui::open_new_tab(&app, &url)
}

/// 切换标签:隐藏当前 webview,显示目标 webview(不重新加载,保留页面状态)
#[tauri::command]
async fn toolbar_switch_tab(
    app: tauri::AppHandle,
    state: State<'_, ui::UiState>,
    id: u32,
) -> Result<(), String> {
    let _ = state;
    ui::switch_tab(&app, id)
}

/// 关闭标签:销毁其 webview,切换到邻近标签(关闭最后一个则自动新建空白标签)
#[tauri::command]
async fn toolbar_close_tab(
    app: tauri::AppHandle,
    state: State<'_, ui::UiState>,
    id: u32,
) -> Result<(), String> {
    let _ = state;
    ui::close_tab(&app, id)
}

/// 地址栏导航:更新激活标签 URL 并导航
#[tauri::command]
async fn toolbar_navigate(
    app: tauri::AppHandle,
    state: State<'_, ui::UiState>,
    url: String,
) -> Result<(), String> {
    if url.is_empty() {
        return Err("url is required".into());
    }
    // 裸域名自动补 http://
    let url = ui::normalize_url(&url);
    let active = *state.active_tab.lock().unwrap();
    {
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(t) = tabs.iter_mut().find(|t| t.id == active) {
            t.url = url.clone();
            t.errored = false;
        }
    }
    // 记录导航前文档身份,用于确认导航确实发生(与 /api/navigate 同口径)
    let prev = ui_block(&app, |h| control::document_state(h))?;
    let prev_url = prev
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let prev_to = prev.get("timeOrigin").and_then(|v| v.as_f64());
    let nav_url = url.clone();
    let nav_res = ui_block(&app, move |h| control::navigate(h, &nav_url));
    if let Err(e) = &nav_res {
        eprintln!("[toolbar] navigate failed: {url} -> {e}");
    }
    nav_res?;
    // 有界等待就绪;未就绪至少留痕(此前静默"成功")
    let wait_url = url.clone();
    match ui_block(&app, move |h| {
        control::wait_ready(h, &prev_url, prev_to, control::nav_timeout_ms())
    }) {
        Ok(st) if st.get("ready").and_then(|v| v.as_bool()) == Some(false) => {
            eprintln!("[toolbar] navigate not ready: {wait_url} -> {st}")
        }
        Err(e) => eprintln!("[toolbar] navigate wait failed: {wait_url} -> {e}"),
        _ => {}
    }
    ui::emit_tabs_changed(&app);
    Ok(())
}

/// 切换面板浮层显示,返回新状态
#[tauri::command]
fn toolbar_toggle_panel(
    app: tauri::AppHandle,
    state: State<'_, ui::UiState>,
) -> Result<bool, String> {
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

/// 页面刷新:错误页(chrome-error:)时重新导航原 URL,否则原生 reload
#[tauri::command]
async fn toolbar_reload(app: tauri::AppHandle) -> Result<(), String> {
    ui_block(&app, |h| ui::reload_active_tab(h).map(|_| ()))
}

fn main() {
    // 显式声明 DPI 感知(否则窗口被系统虚拟化缩放,与 WebView2 真实 DPI 不一致导致布局错位)
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let (browsers_path, port, user_data_dir, notify_url, title, cli_token) = parse_args();

    // 事件流配置(console/network 缓冲与聚合窗口):必须在 create_ui 前确定
    // (create_ui 依此决定是否注入 console 猴补)。
    streams::configure(streams_config_from_args());
    let stream_cfg = streams::config();
    eprintln!(
        "[streams] console={}B network={}B burst={}ms inject={}",
        stream_cfg.console_bytes,
        stream_cfg.network_bytes,
        stream_cfg.burst_window_ms,
        stream_cfg.console_inject
    );

    // HTTP API 鉴权:令牌在实例创建期确定,随实例不可变(attach 模式由插件用同一环境变量提供)。
    // 日志只打前 4 位 + 长度,绝不打印完整令牌。
    let (auth_token, token_source) = resolve_auth_token(cli_token);
    eprintln!(
        "[auth] token source={token_source} prefix={} len={}",
        token_prefix(&auth_token),
        auth_token.chars().count()
    );
    persist_auth_token(user_data_dir.as_deref(), &auth_token);

    tauri::Builder::default()
        .manage(ui::UiState::new())
        // 面板通过 invoke("api_token") 读取同一实例令牌(与 http::serve 的令牌同源)
        .manage(ApiToken(Arc::from(auth_token.as_str())))
        .invoke_handler(tauri::generate_handler![
            panel_records,
            panel_set_note,
            panel_cmd,
            panel_mode,
            panel_service_port,
            api_token,
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
            // 用户数据目录 + 服务端口 + 通知地址:必须在 create_ui 前写好
            // (create_ui 读取端口,注入页面事件上报脚本 window.__btEventPort)
            {
                let state = app.state::<ui::UiState>();
                *state.user_data_dir.lock().unwrap() = user_data_dir.clone();
                *state.service_port.lock().unwrap() = port;
                // 推送式投递:记录插件通知地址(来自 --notify-url / BT_SHELL_NOTIFY_URL;可被 /api/notify-url 覆盖)
                if notify_url.is_some() {
                    *state.notify_url.lock().unwrap() = notify_url.clone();
                }
            }
            // 创建 Webview 布局(工具栏/页面/覆盖层/面板);title 用于多会话实例在任务栏可分辨
            ui::create_ui(app.handle(), port, title.as_deref())?;

            // 启动 HTTP 服务(端口由 --port 指定,默认 0 随机)
            let http_app = Arc::new(bt_shell::service::App::new(
                app.handle().clone(),
                browsers_path,
            ));
            let http_app2 = http_app.clone();
            let http_token = auth_token.clone();
            tauri::async_runtime::spawn(async move {
                let _ = bt_shell::http::serve(http_app2, port, http_token).await;
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

            // 标签状态为事件驱动,无轮询:导航/加载由 on_navigation/on_page_load 更新;
            // SPA/标题/图标由页面脚本上报 /api/page-event(见 ui::PAGE_EVENT_JS)
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
