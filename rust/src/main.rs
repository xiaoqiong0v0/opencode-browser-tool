mod browsers;
mod cdp;
mod http;
mod service;

use std::sync::Arc;

use service::App;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 参数解析(与旧 Node 服务兼容):--browsers-path, --browser, --port
    let mut browsers_path = String::new();
    let mut browser_type = "chromium".to_string();
    let mut port: u16 = 0;
    let mut mirror: Option<String> = None;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--browsers-path" => {
                i += 1;
                if i < args.len() {
                    browsers_path = args[i].clone();
                }
            }
            "--browser" => {
                i += 1;
                if i < args.len() {
                    browser_type = args[i].clone();
                }
            }
            "--port" => {
                i += 1;
                if i < args.len() {
                    port = args[i].parse().unwrap_or(0);
                }
            }
            "--mirror" => {
                i += 1;
                if i < args.len() {
                    mirror = Some(args[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }

    if browsers_path.is_empty() {
        // 默认插件数据目录
        browsers_path = std::env::var("PW_BROWSERS_PATH").unwrap_or_default();
    }
    if browsers_path.is_empty() {
        browsers_path = std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or_default();
    }
    if browsers_path.is_empty() {
        eprintln!("--browsers-path is required");
        std::process::exit(1);
    }

    let app = Arc::new(App::new(browsers_path, mirror, browser_type));
    http::serve(app, port).await.map_err(|e| e.into())
}
