//! HTTP 服务:承载 /api/* 端点,与旧 Node 服务响应格式兼容
//! {success, data} 或 {success: false, error}
use axum::{
    body::Body,
    http::{header, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, post},
    Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::cdp::browser::CdpBrowser;
use crate::cdp::session::{EventHub, PendingResponse, SessionState};
use crate::service::App;

/// 启动 HTTP 服务,端口写入 stdout(与 Node client.ts 协议一致)
pub async fn serve(app: Arc<App>, port: u16) -> Result<(), String> {
    let router = Router::new()
        .route("/api/{*path}", any(handler))
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("bind failed: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    println!("{}", addr.port());
    axum::serve(listener, router)
        .await
        .map_err(|e| format!("serve failed: {e}"))
}

/// 统一入口:解析请求体,分发到 App::handle
async fn handler(
    app: axum::extract::State<Arc<App>>,
    req: Request<Body>,
) -> Response {
    if req.method() == axum::http::Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type")
            .body(Body::empty())
            .unwrap();
    }

    let path = req.uri().path().to_string();
    let bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return err_response(&format!("read body failed: {e}")),
    };
    let body: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };

    let result = app.handle(&path, body).await;
    let resp = match result {
        Ok(v) => json!({ "success": true, "data": v }),
        Err(msg) => json!({ "success": false, "error": msg }),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(resp.to_string()))
        .unwrap()
}

fn err_response(msg: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "success": false, "error": msg }).to_string()))
        .unwrap()
}
