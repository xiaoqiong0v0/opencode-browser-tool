//! HTTP 服务:承载 /api/* 端点,与旧 Node 服务响应格式兼容
//! {success, data} 或 {success: false, error}
//!
//! 鉴权(1.10.0 / 1.11.0):本地 API 只服务插件进程与自家面板,不服务任何外部浏览器页面。
//! - 所有 /api/* 请求必须带 `x-bt-token` 头,与期望令牌做常量时间比较;
//! - 带 `Origin` 头的请求:仅面板自身 origin 白名单放行(见 `PANEL_ORIGINS`),其余一律 403;
//! - 面板 origin 的跨源预检(`OPTIONS`)回 204 + 精确 ACAO,其它 `OPTIONS` 直接 405;
//! - 面板 origin 之外的响应不带任何 CORS 头;
//! - 唯一令牌豁免:精确路径 `/api/page-event` 与 `/api/console-log`(页面注入脚本无法持有令牌,
//!   详见 handler 注释);后者额外限制 body ≤ 256 KiB。
use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{Method, Request, StatusCode, header},
    response::Response,
    routing::any,
};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;

use crate::service::App;

/// 鉴权令牌请求头名(大小写不敏感)
const TOKEN_HEADER: &str = "x-bt-token";

/// 页面事件桥路径:鉴权豁免路径之一(精确匹配,禁止前缀匹配)
const PAGE_EVENT_PATH: &str = "/api/page-event";
/// 页面 console 回传路径(Linux 注入脚本;同样豁免令牌/Origin,只写缓冲)
const CONSOLE_LOG_PATH: &str = "/api/console-log";

/// 普通请求体读取上限(10 MiB);console-log 另有 256 KiB 上限
const MAX_REQUEST_BODY: usize = 10 * 1024 * 1024;

/// 按路径取请求体上限(console-log 提前收紧,避免先读 10 MiB 再拒绝)
fn body_limit_for(path: &str) -> usize {
    if path == CONSOLE_LOG_PATH {
        crate::streams::CONSOLE_UPLOAD_MAX_BYTES
    } else {
        MAX_REQUEST_BODY
    }
}

/// `Content-Length` 是否已声明超限(非法/缺失头不能据此拒绝,交给有界读取兜底)
fn declared_length_too_large(value: Option<&str>, limit: usize) -> bool {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|n| n > limit as u64)
        .unwrap_or(false)
}

/// Chromium「私有网络访问」预检请求头:面板源(`tauri.localhost`)访问 127.0.0.1 时可能带上,
/// 命中则需在预检响应里回 `Access-Control-Allow-Private-Network: true`,否则浏览器会拦掉实际请求
const PNA_REQUEST_HEADER: &str = "access-control-request-private-network";

/// 预检响应里 `Access-Control-Allow-Private-Network` 的响应头名
const PNA_ALLOW_HEADER: &str = "access-control-allow-private-network";

/// 面板自身 origin 白名单(精确形式,不做大小写/端口/子域归一化):
/// - `http://tauri.localhost` / `https://tauri.localhost`:Windows WebView2 下 tauri 自定义协议的源;
/// - `tauri://localhost`:Linux/macOS WebKit 自定义协议下的源。
/// 其余任何 Origin(含 `null`、任意 http(s) 站点、带端口或子域变体)都不在此列。
const PANEL_ORIGINS: [&str; 3] = [
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
];

/// HTTP 服务共享状态:App 句柄 + 期望令牌(实例创建期确定,运行期不可变)
#[derive(Clone)]
struct HttpState {
    app: Arc<App>,
    token: Arc<str>,
}

/// 启动 HTTP 服务,端口写入 stdout(与 Node client.ts 协议一致)
pub async fn serve(app: Arc<App>, port: u16, token: String) -> Result<(), String> {
    let state = HttpState {
        app,
        token: Arc::from(token),
    };
    let router = Router::new()
        .route("/api/{*path}", any(handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("bind failed: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    println!("{}", addr.port());
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| format!("serve failed: {e}"))
}

/// 统一入口:页面事件豁免 → Origin → OPTIONS → 令牌 → 解析请求体 → 分发到 App::handle
async fn handler(
    state: State<HttpState>,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
) -> Response {
    // 页面事件桥唯一豁免(精确匹配 `/api/page-event`,不做前缀匹配):
    // - 页面 JS 无法持有令牌:注入脚本运行在远程页面源下,拿不到 shell 的 `x-bt-token`;
    // - 浏览器跨源请求必带 `Origin`(no-cors 也带,只是不带响应给页面),故必须同时豁免 Origin 校验。
    // 残余风险:该端点只把 url/title/icon 写进 TabState 提示(不执行任何操作、无敏感数据回读),
    // 因此被同机页面伪造上报最多造成地址栏/标题显示错乱。若未来给该端点加敏感字段或引入副作用,
    // 必须重新评估此豁免(改为短时效签名/一次性 nonce 等)。
    //
    // 面板 origin 放行(1.11.0)的理由与残余风险:
    // - 面板 UI 由本进程的 webview 加载,源固定为 `tauri.localhost`(Windows WebView2)或
    //   `tauri://localhost`(Linux/macOS),面板 `fetch` 必然带其中之一;1.10.0 的一律拒绝
    //   把自家面板也挡在门外(面板 `/api/*` 全 403,设备预设下拉只剩「默认」)。
    // - 外部网页无法伪造 `Origin`(浏览器强制写入),`null`/带端口/子域变体都不在白名单;
    //   放行不扩大攻击面:面板 origin 的请求仍需 `x-bt-token`,令牌才是真正的闸门,Origin 只做纵深防御。
    // - 残余风险:同机进程可用任意 Origin 自行构造请求(绕过 Origin 语义),但同机进程本就能读到
    //   实例令牌,故不构成新增风险;若将来令牌改为不可被同机进程读取,需重新评估本白名单。
    let path = req.uri().path();
    // 面板精确 origin(命中白名单才 Some):供预检与响应 CORS 头复用
    let panel_origin: Option<String> = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|s| is_panel_origin(s.as_bytes()))
        .map(str::to_string);
    let has_origin = req.headers().contains_key(header::ORIGIN);
    let token_ok = token_matches(&req, &state.token);

    // 判定顺序固定:页面事件豁免 → Origin → OPTIONS → 令牌(见 decide_auth)
    match decide_auth(
        req.method(),
        path,
        has_origin,
        panel_origin.is_some(),
        token_ok,
    ) {
        AuthDecision::Allow => {}
        AuthDecision::ForbiddenOrigin => return auth_error(StatusCode::FORBIDDEN),
        AuthDecision::OptionsNotAllowed => return method_not_allowed(),
        AuthDecision::Unauthorized => {
            return attach_cors(
                auth_error(StatusCode::UNAUTHORIZED),
                panel_origin.as_deref(),
            );
        }
        // 面板跨源预检:必须回 CORS 头,否则浏览器直接拦掉后续实际请求
        // (面板用 `Content-Type: application/json` + 自定义 `x-bt-token`,都属非简单请求,必触发预检)
        AuthDecision::Preflight => {
            let origin = panel_origin.as_deref().unwrap_or_default();
            let pna_requested = req
                .headers()
                .get(PNA_REQUEST_HEADER)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.eq_ignore_ascii_case("true"));
            return preflight_response(origin, pna_requested);
        }
    }

    let path = path.to_string();
    let body_limit = body_limit_for(&path);
    // 页面注入回传先看 `Content-Length`:超限直接 413,不先把 10 MiB 读进来
    if path == CONSOLE_LOG_PATH {
        let declared = req
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok());
        if declared_length_too_large(declared, body_limit) {
            return payload_too_large("console-log body too large (max 256 KiB)");
        }
    }
    // 无 `Content-Length`(chunked)时同样用有界读取,超限在读取阶段直接拒绝
    let bytes = match axum::body::to_bytes(req.into_body(), body_limit).await {
        Ok(b) => b,
        Err(e) => {
            if path == CONSOLE_LOG_PATH {
                return payload_too_large("console-log body too large (max 256 KiB)");
            }
            return err_response(&format!("read body failed: {e}"));
        }
    };
    let body: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };

    let result = state.app.handle(&path, body, Some(client_addr.ip())).await;
    let resp = match result {
        Ok(v) => json!({ "success": true, "data": v }),
        Err(msg) => json!({ "success": false, "error": msg }),
    };
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(resp.to_string()))
        .unwrap();
    attach_cors(resp, panel_origin.as_deref())
}

/// 是否为面板自身 origin(精确字节比较,不放宽大小写/端口/子域)
fn is_panel_origin(origin: &[u8]) -> bool {
    PANEL_ORIGINS.iter().any(|o| o.as_bytes() == origin)
}

/// 单次请求的鉴权判定结果
#[derive(Debug, PartialEq, Eq)]
enum AuthDecision {
    /// 放行:继续解析请求体并分发
    Allow,
    /// 面板跨源预检:返回 CORS 预检响应
    Preflight,
    /// 带非面板 Origin:403
    ForbiddenOrigin,
    /// 非面板(或无 Origin)的 OPTIONS:405
    OptionsNotAllowed,
    /// 令牌缺失/不匹配:401
    Unauthorized,
}

/// 页面桥路径(`/api/page-event`、`/api/console-log`):令牌 + Origin 双豁免
fn is_page_bridge_path(path: &str) -> bool {
    path == PAGE_EVENT_PATH || path == CONSOLE_LOG_PATH
}

/// 纯函数形式的鉴权判定,固定顺序:页面事件豁免 → Origin → OPTIONS → 令牌
///
/// - `has_origin`:请求是否带 `Origin` 头(即便值非 UTF-8 也算带)
/// - `panel_origin`:Origin 值是否精确命中面板白名单
fn decide_auth(
    method: &Method,
    path: &str,
    has_origin: bool,
    panel_origin: bool,
    token_ok: bool,
) -> AuthDecision {
    let is_page_event = is_page_bridge_path(path);
    // 页面事件豁免最先判定:它既不带令牌,也不受 Origin 限制
    if !is_page_event && has_origin && !panel_origin {
        return AuthDecision::ForbiddenOrigin;
    }
    if method == Method::OPTIONS {
        return if panel_origin {
            AuthDecision::Preflight
        } else {
            AuthDecision::OptionsNotAllowed
        };
    }
    if !is_page_event && !token_ok {
        return AuthDecision::Unauthorized;
    }
    AuthDecision::Allow
}

/// 面板跨源预检响应:只回放行的那种精确 origin,绝不回通配 `*`
///
/// `pna_requested`:请求带 `Access-Control-Request-Private-Network: true` 时补回对应允许头,
/// 兼容 Chromium 对「面板源 → 127.0.0.1」的私有网络预检。
fn preflight_response(origin: &str, pna_requested: bool) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "POST, OPTIONS")
        .header(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            "content-type, x-bt-token",
        )
        .header(header::ACCESS_CONTROL_MAX_AGE, "600")
        .header(header::VARY, "Origin");
    if pna_requested {
        builder = builder.header(PNA_ALLOW_HEADER, "true");
    }
    builder.body(Body::empty()).unwrap()
}

/// 给响应附加面板 CORS 头(仅面板自身 origin;其它请求不加任何 CORS 头)
fn attach_cors(mut resp: Response, panel_origin: Option<&str>) -> Response {
    let Some(origin) = panel_origin else {
        return resp;
    };
    if let Ok(value) = header::HeaderValue::from_str(origin) {
        let headers = resp.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        headers.insert(header::VARY, header::HeaderValue::from_static("Origin"));
    }
    resp
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

/// 非面板 origin 的 OPTIONS 不支持(不回任何 CORS 头)
fn method_not_allowed() -> Response {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "success": false, "error": "method_not_allowed" }).to_string(),
        ))
        .unwrap()
}

/// 请求体超限:413(与普通业务错误区分,便于注入脚本侧识别)
fn payload_too_large(msg: &str) -> Response {
    Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "success": false, "error": msg }).to_string(),
        ))
        .unwrap()
}

fn err_response(msg: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "success": false, "error": msg }).to_string(),
        ))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::{
        AuthDecision, PAGE_EVENT_PATH, attach_cors, decide_auth, is_panel_origin,
        preflight_response,
    };
    use axum::{
        body::Body,
        http::{Method, StatusCode, header},
        response::Response,
    };

    /// 三种面板 origin 精确放行;其余形式(evil / null / 子域前缀 / 带端口)全部拒绝
    #[test]
    fn panel_origin_whitelist_is_exact() {
        for ok in [
            "http://tauri.localhost",
            "https://tauri.localhost",
            "tauri://localhost",
        ] {
            assert!(is_panel_origin(ok.as_bytes()), "should accept: {ok}");
        }
        for bad in [
            "http://evil.com",
            "null",
            "http://tauri.localhost.evil.com",
            "http://localhost:3000",
            // 端口/大小写/尾部斜杠变体都不在白名单
            "http://tauri.localhost:3000",
            "HTTP://TAURI.LOCALHOST",
            "http://tauri.localhost/",
            "tauri://localhost.evil.com",
        ] {
            assert!(!is_panel_origin(bad.as_bytes()), "must reject: {bad}");
        }
    }

    /// 非面板 Origin 先于 OPTIONS/令牌判定:403(即便带合法令牌、即便方法是 OPTIONS)
    #[test]
    fn foreign_origin_rejected_before_options_and_token() {
        for method in [Method::POST, Method::OPTIONS, Method::GET] {
            assert_eq!(
                decide_auth(&method, "/api/device/list", true, false, true),
                AuthDecision::ForbiddenOrigin
            );
        }
    }

    /// 面板 origin 的预检走 CORS 204,不受令牌约束(预检本来就不带令牌)
    #[test]
    fn panel_preflight_does_not_require_token() {
        assert_eq!(
            decide_auth(&Method::OPTIONS, "/api/device/list", true, true, false),
            AuthDecision::Preflight
        );
    }

    /// 面板 origin 的实际请求仍需令牌:缺失/错误 → 401
    #[test]
    fn panel_origin_still_requires_token() {
        assert_eq!(
            decide_auth(&Method::POST, "/api/device/list", true, true, false),
            AuthDecision::Unauthorized
        );
        assert_eq!(
            decide_auth(&Method::POST, "/api/device/list", true, true, true),
            AuthDecision::Allow
        );
    }

    /// 无 Origin(插件 Node fetch)+ 合法令牌 → 放行;无令牌 → 401
    #[test]
    fn no_origin_requires_token() {
        assert_eq!(
            decide_auth(&Method::POST, "/api/device/list", false, false, true),
            AuthDecision::Allow
        );
        assert_eq!(
            decide_auth(&Method::POST, "/api/device/list", false, false, false),
            AuthDecision::Unauthorized
        );
    }

    /// 无 Origin / 非面板 origin 的 OPTIONS → 405(不再支持通配 CORS 预检)
    #[test]
    fn foreign_or_missing_origin_options_not_allowed() {
        assert_eq!(
            decide_auth(&Method::OPTIONS, "/api/device/list", false, false, false),
            AuthDecision::OptionsNotAllowed
        );
        assert_eq!(
            decide_auth(&Method::OPTIONS, "/api/device/list", true, false, false),
            AuthDecision::ForbiddenOrigin
        );
    }

    /// page-event / console-log 豁免最先生效:非面板 Origin、无令牌也能放行(但 OPTIONS 仍 405)
    #[test]
    fn page_bridge_exemption_wins_first() {
        for p in [PAGE_EVENT_PATH, "/api/console-log"] {
            assert_eq!(
                decide_auth(&Method::POST, p, true, false, false),
                AuthDecision::Allow
            );
            assert_eq!(
                decide_auth(&Method::OPTIONS, p, true, false, false),
                AuthDecision::OptionsNotAllowed
            );
        }
        // 前缀相似路径不在豁免范围
        assert_eq!(
            decide_auth(&Method::POST, "/api/page-event/x", true, false, false),
            AuthDecision::ForbiddenOrigin
        );
        assert_eq!(
            decide_auth(&Method::POST, "/api/console-log/x", true, false, false),
            AuthDecision::ForbiddenOrigin
        );
    }

    /// 只有面板 origin 的响应才回 ACAO + Vary;其它请求不带 CORS 头
    #[test]
    fn cors_headers_only_for_panel_origin() {
        let resp = attach_cors(Response::new(Body::empty()), Some("http://tauri.localhost"));
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            Some("http://tauri.localhost")
        );
        assert_eq!(
            resp.headers()
                .get(header::VARY)
                .and_then(|v| v.to_str().ok()),
            Some("Origin")
        );

        let resp = attach_cors(Response::new(Body::empty()), None);
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    /// 预检响应:204 + 精确 origin 回显 + 允许 x-bt-token 头,绝不通配
    #[test]
    fn preflight_echoes_exact_origin() {
        let resp = preflight_response("tauri://localhost", false);
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            Some("tauri://localhost")
        );
        let allow_headers = resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(allow_headers.contains("x-bt-token"), "got: {allow_headers}");
        assert!(
            allow_headers.contains("content-type"),
            "got: {allow_headers}"
        );
        // 未请求私有网络访问时不回该头(保持最小授予)
        assert!(resp.headers().get(super::PNA_ALLOW_HEADER).is_none());
    }

    /// Chromium 私有网络预检:请求要求时补回允许头
    #[test]
    fn preflight_allows_private_network_only_when_requested() {
        let resp = preflight_response("http://tauri.localhost", true);
        assert_eq!(
            resp.headers()
                .get(super::PNA_ALLOW_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some("true")
        );
    }

    /// console-log 用 256 KiB 上限,其它端点 10 MiB;Content-Length 提前判定超限
    #[test]
    fn console_upload_body_limits_are_checked_early() {
        use super::{body_limit_for, declared_length_too_large};

        assert_eq!(
            body_limit_for("/api/console-log"),
            crate::streams::CONSOLE_UPLOAD_MAX_BYTES
        );
        assert_eq!(body_limit_for("/api/network-logs"), 10 * 1024 * 1024);

        let console_limit = crate::streams::CONSOLE_UPLOAD_MAX_BYTES;
        assert!(!declared_length_too_large(None, console_limit));
        assert!(!declared_length_too_large(Some("not-a-number"), console_limit));
        assert!(!declared_length_too_large(
            Some(&(console_limit as u64).to_string()),
            console_limit
        ));
        assert!(declared_length_too_large(
            Some(&(console_limit as u64 + 1).to_string()),
            console_limit
        ));
    }

    /// 413 响应体与状态码符合约定
    #[test]
    fn payload_too_large_is_413() {
        let resp = super::payload_too_large("too big");
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
