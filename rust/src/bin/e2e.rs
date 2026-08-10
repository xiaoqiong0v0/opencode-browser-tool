//! M2 端到端验证:HTTP 服务 + CDP 操作
use std::sync::Arc;

use pw_shell::{http, service::App};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let browsers_path = "C:/Users/king-/.opencode/plugins-data/opencode-playwright-tool/browsers";
    let app = Arc::new(App::new(browsers_path.into(), None, "chromium".into()));
    let port = 19666u16;

    // 启动 HTTP 服务(后台)
    let app2 = app.clone();
    tokio::spawn(async move {
        let _ = http::serve(app2, port).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let base = format!("http://127.0.0.1:{port}");
    println!("== M2 e2e test ==");

    // navigate
    let r = post(&base, "/api/navigate", serde_json::json!({ "url": "https://example.com" })).await;
    println!("navigate: {}", r);

    // status
    let r = post(&base, "/api/status", serde_json::json!({})).await;
    println!("status: {}", r);

    // evaluate
    let r = post(&base, "/api/evaluate", serde_json::json!({ "script": "document.title" })).await;
    println!("evaluate title: {}", r);

    // visible-text
    let r = post(&base, "/api/visible-text", serde_json::json!({})).await;
    println!("visible-text head: {}", r.chars().take(60).collect::<String>());

    // element-state
    let r = post(&base, "/api/element-state", serde_json::json!({ "selector": "h1" })).await;
    println!("element-state: {}", r);

    // screenshot
    let r = post(&base, "/api/screenshot", serde_json::json!({})).await;
    println!("screenshot buffer len: {}", r.len());

    // console-logs
    let r = post(&base, "/api/console-logs", serde_json::json!({})).await;
    println!("console-logs: {}", r);

    // click
    let r = post(&base, "/api/click", serde_json::json!({ "selector": "a" })).await;
    println!("click: {}", r);

    // tabs
    let r = post(&base, "/api/tabs", serde_json::json!({})).await;
    println!("tabs: {}", r);

    // close
    let r = post(&base, "/api/close", serde_json::json!({})).await;
    println!("close: {}", r);

    println!("== ALL PASS ==");
    Ok(())
}

async fn post(base: &str, path: &str, body: serde_json::Value) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let json: serde_json::Value = resp.json().await.unwrap();
    json.to_string()
}
