//! pw-shell 主入口:Tauri 应用(跨平台多 Webview)
//! 单窗口:页面 Webview + 覆盖层 Webview + 面板 Webview
//! 启动参数兼容旧协议:--browsers-path, --browser, --session-isolation, --port
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use pw_shell::ui::annotate::Annotator;
use pw_shell::ui::{self, AnnotationRecord};
use tauri::{Listener, Manager, State};

/// 解析命令行参数(兼容旧 Node 服务协议)
fn parse_args() -> (String, u16) {
    let args: Vec<String> = std::env::args().collect();
    let mut browsers_path = String::new();
    let mut port: u16 = 0;
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
            _ => {}
        }
        i += 1;
    }
    (browsers_path, port)
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
        "send-all" => {
            // 发送批注:记录快照入队,插件端轮询 consume
            let records = state.records.lock().unwrap().clone();
            let count = records.len();
            let items: Vec<serde_json::Value> = records
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "index": r.index,
                        "selector": r.selector,
                        "rect": [r.rect.0, r.rect.1, r.rect.2, r.rect.3],
                        "note": r.note,
                    })
                })
                .collect();
            *state.sent_records.lock().unwrap() = items;
            Ok(count > 0)
        }
        _ => Err(format!("unknown panel cmd: {cmd}")),
    }
}

fn main() {
    // 显式声明 DPI 感知(否则窗口被系统虚拟化缩放,与 WebView2 真实 DPI 不一致导致布局错位)
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let (browsers_path, port) = parse_args();

    tauri::Builder::default()
        .manage(ui::UiState::new())
        .invoke_handler(tauri::generate_handler![
            panel_records,
            panel_set_note,
            panel_cmd
        ])
        .setup(move |app| {
            // 创建三 Webview 布局
            ui::create_ui(app.handle())?;

            // 启动 HTTP 服务(端口由 --port 指定,默认 0 随机)
            let http_app = Arc::new(pw_shell::service::App::new(
                app.handle().clone(),
                browsers_path,
            ));
            let http_app2 = http_app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = pw_shell::http::serve(http_app2, port).await;
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
