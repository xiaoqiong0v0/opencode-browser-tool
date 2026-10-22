//! HTTP 服务:承载 /api/* 端点,与旧 Node 服务响应格式兼容
//! {success, data} 或 {success: false, error}
//!
//! 鉴权(1.10.0):本地 API 只服务插件进程,不服务任何浏览器页面。
//! - 所有 /api/* 请求必须带 `x-bt-token` 头,与期望令牌做常量时间比较;
//! - 带 `Origin` 头的请求一律 403(浏览器跨源必带,插件 Node fetch 不带);
//! - 不再返回任何 CORS 响应头,`OPTIONS` 直接 405;
//! - 唯一例外:精确路径 `/api/page-event`(页面注入脚本无法持有令牌,详见 handler 注释)。
use axum::{
    body::Body,
    extract::State,
    http::{header, Method, Request, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::service::App;

/// 鉴权令牌请求头名(大小写不敏感)
const TOKEN_HEADER: &str = "x-bt-token";

/// 页面事件桥路径:唯一的鉴权豁免路径(精确匹配,禁止前缀匹配)
const PAGE_EVENT_PATH: &str = "/api/page-event";

/// HTTP 服务共享状态:App 句柄 + 期望令牌(实例创建期确定,运行期不可变)
#[derive(Clone)]
struct HttpState {
    app: Arc<App>,
    token: Arc<str>,
}

/// 启动 HTTP 服务,端口写入 stdout(与 Node client.ts 协议一致)
pub async fn serve(app: Arc<App>, port: u16, token: String) -> Result<(), String> {
    let state = HttpState { app, token: Arc::from(token) };
    let router = Router::new()
        .route("/api/{*path}", any(handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("bind failed: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    println!("{}", addr.port());
    axum::serve(listener, router)
        .await
        .map_err(|e| format!("serve failed: {e}"))
}

/// 统一入口:鉴权 → 解析请求体 → 分发到 App::handle
async fn handler(state: State<HttpState>, req: Request<Body>) -> Response {
    // 页面事件桥唯一豁免(精确匹配 `/api/page-event`,不做前缀匹配):
    // - 页面 JS 无法持有令牌:注入脚本运行在远程页面源下,拿不到 shell 的 `x-bt-token`;
    // - 浏览器跨源请求必带 `Origin`(no-cors 也带,只是不带响应给页面),故必须同时豁免 Origin 校验。
    // 残余风险:该端点只把 url/title/icon 写进 TabState 提示(不执行任何操作、无敏感数据回读),
    // 因此被同机页面伪造上报最多造成地址栏/标题显示错乱。若未来给该端点加敏感字段或引入副作用,
    // 必须重新评估此豁免(改为短时效签名/一次性 nonce 等)。
    let path = req.uri().path();
    let is_page_event = path == PAGE_EVENT_PATH;

    // 浏览器发起的跨源请求必带 Origin:本地 API 不面向浏览器页面,一律拒绝(先于 OPTIONS 判定)。
    if !is_page_event && req.headers().contains_key(header::ORIGIN) {
        return auth_error(StatusCode::FORBIDDEN);
    }
    // 不再支持 CORS 预检(豁免路径也不放宽)
    if req.method() == Method::OPTIONS {
        return method_not_allowed();
    }
    if !is_page_event && !token_matches(&req, &state.token) {
        return auth_error(StatusCode::UNAUTHORIZED);
    }

    let path = path.to_string();
    let bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return err_response(&format!("read body failed: {e}")),
    };
    let body: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };

    let result = state.app.handle(&path, body).await;
    let resp = match result {
        Ok(v) => json!({ "success": true, "data": v }),
        Err(msg) => json!({ "success": false, "error": msg }),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(resp.to_string()))
        .unwrap()
}

/// 校验请求头里的令牌:缺失/非 UTF-8 视为不匹配
fn token_matches(req: &Request<Body>, expected: &str) -> bool {
    match req.headers().get(TOKEN_HEADER) {
        Some(v) => constant_time_eq(v.as_bytes(), expected.as_bytes()),
        None => false,
    }
}

/// 常量时间字符串比较:长度不等也遍历到较长者,不用提前返回避免时序泄漏
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max_len = a.len().max(b.len());
    let mut diff = (a.len() ^ b.len()) as u32;
    for i in 0..max_len {
        // 越界位置按 0 参与异或(长度差异已并入 diff)
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= (x ^ y) as u32;
    }
    diff == 0
}

/// 鉴权失败响应(401/403):统一 unauthorized,**不回显期望令牌**
fn auth_error(status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "success": false, "error": "unauthorized" }).to_string(),
        ))
        .unwrap()
}

/// OPTIONS 不再支持(CORS 已移除)
fn method_not_allowed() -> Response {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "success": false, "error": "method_not_allowed" }).to_string(),
        ))
        .unwrap()
}

fn err_response(msg: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "success": false, "error": msg }).to_string()))
        .unwrap()
}
