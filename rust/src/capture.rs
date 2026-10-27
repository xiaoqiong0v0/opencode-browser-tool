//! 控制台 / 网络事件采集(1.12.0)
//!
//! - Windows:WebView2 CDP(`Runtime.enable` + `Log.enable` + `Network.enable` + `Page.enable`),
//!   经 `ICoreWebView2DevToolsProtocolEventReceiver` 订阅事件,写入 [`crate::streams`];
//! - Linux:console 由页面注入猴补回传 `/api/console-log`(见 [`CONSOLE_INJECT_JS`]),
//!   network 走 WebKitGTK `resource-load-started` + `notify::response` / `failed`;
//! - 相位:`Page.frameNavigated` / `Page.loadEventFired`(Windows),
//!   `load-changed` STARTED/FINISHED(Linux)。
//!
//! 线程纪律:WebView2 事件回调在 UI 线程执行,只做 JSON 解析 + 短临界区写入,
//! 禁止任何 sleep / 阻塞等待 / 重活。
use tauri::AppHandle;

use crate::streams::{self, NetworkEvent};
#[cfg(windows)]
use crate::streams::{ConsoleEvent, ConsoleSource};

/// Linux 页面注入:控制台猴补(console 五级 + onerror + unhandledrejection +
/// 1s 重挂检测 + 同源 iframe postMessage 汇总 + 300ms/50 条批量回传)。
/// `window.__btEventPort` / `window.__btPageLabel` 由 Rust 在页面初始化脚本前缀注入。
/// 契约:`source=injected`;单批 body ≤ 256 KiB;服务端按 IP+tab 限速 200 条/秒。
#[cfg(target_os = "linux")]
pub const CONSOLE_INJECT_JS: &str = r##"
(function(){
  if(window.__btConsoleHooked)return;
  if(!window.__btEventPort)return;
  if(typeof console==="undefined")return;
  window.__btConsoleHooked=true;
  var P=window.__btEventPort;
  var isTop=(window===window.top);
  var q=[];
  var QMAX=500;
  var timer=null;
  function now(){return Date.now();}
  function fmt(args){
    var out=[];
    for(var i=0;i<args.length;i++){
      var a=args[i];
      try{
        if(typeof a==="string")out.push(a);
        else if(a===null)out.push("null");
        else if(typeof a==="object")out.push(JSON.stringify(a));
        else out.push(String(a));
      }catch(e){out.push(String(a));}
    }
    return out.join(" ");
  }
  function enqueue(level,msg,stack){
    try{
      if(!msg)msg="";
      msg=String(msg).slice(0,4096);
      if(q.length>=QMAX)q.shift();
      var e={level:level,msg:msg,ts:now()};
      if(stack)e.stack=String(stack).slice(0,8192);
      q.push(e);
      schedule();
    }catch(e){}
  }
  function schedule(){
    if(timer)return;
    timer=setTimeout(function(){timer=null;flush();},300);
  }
  function flush(){
    if(!q.length)return;
    // 批量上限 50 条,同时按字符预算 56 KiB 切分(fetch keepalive 的 64 KiB 限制)
    var batch=[],size=0;
    while(q.length&&batch.length<50){
      var it=q[0];
      var est=(it.msg||"").length+(it.stack||"").length+80;
      if(batch.length&&size+est>56*1024)break;
      batch.push(q.shift());size+=est;
    }
    if(!isTop){
      try{parent.postMessage({__btConsole:1,entries:batch},"*");}catch(e){}
      if(q.length)schedule();
      return;
    }
    var body="";
    try{body=JSON.stringify({label:window.__btPageLabel||"",entries:batch});}catch(e){return;}
    var url="http://127.0.0.1:"+P+"/api/console-log";
    var sent=false;
    try{
      fetch(url,{method:"POST",body:body,mode:"no-cors",keepalive:true}).then(function(){},function(){});
      sent=true;
    }catch(e){}
    if(!sent){
      try{navigator.sendBeacon(url,body);}catch(e){}
    }
    if(q.length)schedule();
  }
  function wrap(level){
    var cur=console[level];
    if(cur&&cur.__btWrapped)return;
    var orig=cur?cur.bind(console):function(){};
    var w=function(){
      enqueue(level,fmt(arguments));
      try{orig.apply(null,arguments);}catch(e){}
    };
    w.__btWrapped=true;
    console[level]=w;
  }
  ["log","info","warn","error","debug"].forEach(wrap);
  // 1s 重挂检测:页面/框架替换了 console.* 后重新包裹
  setInterval(function(){
    try{["log","info","warn","error","debug"].forEach(wrap);}catch(e){}
  },1000);
  addEventListener("error",function(e){
    try{
      var msg=(e&&e.message)||"script error";
      var stack=(e&&e.error&&e.error.stack)||((e&&e.filename)||"")+":"+((e&&e.lineno)||0)+":"+((e&&e.colno)||0);
      enqueue("error",msg,stack);
    }catch(_){}
  },false);
  addEventListener("unhandledrejection",function(e){
    try{
      var r=e&&e.reason;
      var msg=(r&&r.message)?r.message:String(r);
      var stack=(r&&r.stack)||"";
      enqueue("error","Unhandled rejection: "+msg,stack);
    }catch(_){}
  },false);
  addEventListener("message",function(e){
    try{
      var d=e&&e.data;
      if(d&&d.__btConsole&&d.entries&&d.entries.length){
        for(var i=0;i<d.entries.length;i++){
          var it=d.entries[i]||{};
          enqueue(String(it.level||"log"),String(it.msg||""),it.stack||"");
        }
      }
    }catch(_){}
  },false);
})();
"##;

/// 是否注入 Linux console 猴补(Windows 走 CDP,默认关)
pub fn console_inject_enabled() -> bool {
    cfg!(target_os = "linux") && streams::config().console_inject
}

/// 注册页面采集(每个 page webview 创建后调用一次)
pub fn setup_page(app: &AppHandle, webview: &tauri::webview::Webview, label: &str) {
    #[cfg(windows)]
    windows_cdp::setup(app, webview, label);
    #[cfg(target_os = "linux")]
    linux_webkit::setup(app, webview, label);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (app, webview, label);
    }
}

/// 释放某 webview label 的 CDP 订阅持有(关闭/销毁 webview 后调用)
pub fn release_webview(label: &str) {
    #[cfg(windows)]
    windows_cdp::release(label);
    #[cfg(not(windows))]
    {
        let _ = label;
    }
}

// ---------------------------------------------------------------------------
// Windows:WebView2 CDP
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod windows_cdp {
    use super::*;
    use std::sync::Mutex;

    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceivedEventArgs,
        ICoreWebView2DevToolsProtocolEventReceiver,
    };
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    };
    use windows::core::HSTRING;

    /// 事件种类(订阅时确定,回调里按种类解析)
    #[derive(Clone, Copy)]
    enum Kind {
        ConsoleApi,
        LogEntry,
        Exception,
        FrameNavigated,
        LoadEvent,
        NetRequest,
        NetResponse,
        NetFailed,
        NetFinished,
    }

    /// 持有事件接收器,否则 WebView2 可能回收后不再投递事件。
    /// 只在 UI 线程创建/使用;`Send` 仅为满足 `static Mutex` 的编译要求(见模块注释)。
    struct Holder {
        label: String,
        // 仅用于持有:字段本身不需要读取(读取即触发 dead_code)
        #[allow(dead_code)]
        receivers: Vec<ICoreWebView2DevToolsProtocolEventReceiver>,
    }
    unsafe impl Send for Holder {}

    static HOLDERS: Mutex<Vec<Holder>> = Mutex::new(Vec::new());

    /// 每个页面 webview 的主帧 id(首次 `Page.frameNavigated` 记录,用于区分 iframe 导航)
    static MAIN_FRAME_IDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    /// 向页面注册 CDP 事件与域开关
    pub(super) fn setup(app: &AppHandle, webview: &tauri::webview::Webview, label: &str) {
        let app = app.clone();
        let label = label.to_string();
        let _ = webview.with_webview(move |platform_webview| {
            let controller = platform_webview.controller();
            let Ok(wv) = (unsafe { controller.CoreWebView2() }) else {
                return;
            };
            // 开启采集域(失败不致命:对应事件不投递而已)
            for method in [
                "Runtime.enable",
                "Log.enable",
                "Network.enable",
                "Page.enable",
            ] {
                enable_method(&wv, method);
            }
            let subs: [(&str, Kind); 9] = [
                ("Runtime.consoleAPICalled", Kind::ConsoleApi),
                ("Log.entryAdded", Kind::LogEntry),
                ("Runtime.exceptionThrown", Kind::Exception),
                ("Page.frameNavigated", Kind::FrameNavigated),
                ("Page.loadEventFired", Kind::LoadEvent),
                ("Network.requestWillBeSent", Kind::NetRequest),
                ("Network.responseReceived", Kind::NetResponse),
                ("Network.loadingFailed", Kind::NetFailed),
                ("Network.loadingFinished", Kind::NetFinished),
            ];
            let mut receivers = Vec::new();
            for (event, kind) in subs {
                let Ok(receiver) =
                    (unsafe { wv.GetDevToolsProtocolEventReceiver(&HSTRING::from(event)) })
                else {
                    continue;
                };
                let app = app.clone();
                let label = label.clone();
                let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
                    move |_sender: Option<ICoreWebView2>,
                          args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>|
                          -> windows::core::Result<()> {
                        if let Some(args) = args {
                            let mut raw = windows::core::PWSTR::null();
                            // 参数 JSON 由 WebView2 分配,交回 take_pwstr 释放
                            if unsafe { args.ParameterObjectAsJson(&mut raw) }.is_ok() {
                                let text = webview2_com::take_pwstr(raw);
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                    handle_event(&app, &label, kind, &v);
                                }
                            }
                        }
                        Ok(())
                    },
                ));
                let mut token: i64 = 0;
                if unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }
                    .is_ok()
                {
                    receivers.push(receiver);
                }
            }
            if !receivers.is_empty() {
                let mut holders = HOLDERS.lock().unwrap_or_else(|e| e.into_inner());
                holders.retain(|h| h.label != label);
                holders.push(Holder {
                    label: label.clone(),
                    receivers,
                });
            }
        });
    }

    /// 释放某 label 的订阅(关闭 webview 后)
    pub(super) fn release(label: &str) {
        let mut holders = HOLDERS.lock().unwrap_or_else(|e| e.into_inner());
        holders.retain(|h| h.label != label);
        let mut mains = MAIN_FRAME_IDS.lock().unwrap_or_else(|e| e.into_inner());
        mains.retain(|(l, _)| l != label);
    }

    /// `Page.frameNavigated` 是否来自主帧
    ///
    /// CDP 主帧没有 `parentId`;同时记录该 label 的主帧 id,兼容 OOPIF 等
    /// 子帧上报缺 `parentId` 的边缘情况(只有 `frameId == 主帧 id` 才算)。
    fn is_main_frame(label: &str, v: &serde_json::Value) -> bool {
        let Some(frame) = v.get("frame") else {
            return false;
        };
        let Some(id) = frame.get("id").and_then(|i| i.as_str()) else {
            return false;
        };
        let has_parent = frame.get("parentId").and_then(|p| p.as_str()).is_some();
        let mut mains = MAIN_FRAME_IDS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, known)) = mains.iter().find(|(l, _)| l == label) {
            if !has_parent {
                // 主帧再次导航:刷新记录
                if let Some(slot) = mains.iter_mut().find(|(l, _)| l == label) {
                    slot.1 = id.to_string();
                }
                return true;
            }
            return id == known;
        }
        if has_parent {
            return false;
        }
        mains.push((label.to_string(), id.to_string()));
        true
    }

    /// 调用无参 CDP 方法(域开关)
    fn enable_method(wv: &ICoreWebView2, method: &str) {
        let completed = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
            |_result: windows::core::Result<()>, _json: String| Ok(()),
        ));
        unsafe {
            let _ = wv.CallDevToolsProtocolMethod(
                &HSTRING::from(method),
                &HSTRING::from("{}"),
                &completed,
            );
        }
    }

    /// 分发一条 CDP 事件(UI 线程;只做解析 + 短临界区写入)
    fn handle_event(app: &AppHandle, label: &str, kind: Kind, v: &serde_json::Value) {
        let tab = crate::ui::tab_id_for_label(app, label).unwrap_or(0);
        match kind {
            Kind::ConsoleApi => {
                let level = streams::normalize_console_level(
                    v.get("type").and_then(|t| t.as_str()).unwrap_or("log"),
                );
                let msg = args_to_text(v.get("args"));
                let stack = stack_trace(v.get("stackTrace"));
                streams::push_console(ConsoleEvent {
                    level,
                    msg,
                    ts: streams::now_ms(),
                    phase: String::new(),
                    source: ConsoleSource::Cdp,
                    stack,
                    tab,
                    is_exception: false,
                    internal: false,
                });
            }
            Kind::LogEntry => {
                let Some(entry) = v.get("entry") else { return };
                let level = streams::normalize_console_level(
                    entry
                        .get("level")
                        .and_then(|t| t.as_str())
                        .unwrap_or("info"),
                );
                let msg = entry
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let stack = stack_trace(entry.get("stackTrace"));
                // 浏览器内部日志(`source=violation|deprecation`,如 `[DOM] Password field ...`):
                // 保留在缓冲里便于取证,但默认不返回给模型(`console_logs --include-internal` 才显示)
                let source = entry.get("source").and_then(|t| t.as_str()).unwrap_or("");
                let internal = !matches!(source, "javascript" | "network" | "security" | "xml");
                streams::push_console(ConsoleEvent {
                    level,
                    msg,
                    ts: streams::now_ms(),
                    phase: String::new(),
                    source: ConsoleSource::Cdp,
                    stack,
                    tab,
                    is_exception: false,
                    internal,
                });
            }
            Kind::Exception => {
                let Some(details) = v.get("exceptionDetails") else {
                    return;
                };
                let msg = details
                    .get("exception")
                    .and_then(|e| e.get("description"))
                    .and_then(|d| d.as_str())
                    .or_else(|| details.get("text").and_then(|t| t.as_str()))
                    .unwrap_or("Uncaught exception")
                    .to_string();
                let stack = stack_trace(details.get("stackTrace"));
                streams::push_console(ConsoleEvent {
                    level: "error".into(),
                    msg,
                    ts: streams::now_ms(),
                    phase: String::new(),
                    source: ConsoleSource::Cdp,
                    stack,
                    tab,
                    is_exception: true,
                    internal: false,
                });
            }
            // 只有主帧导航才回到 load:子帧(iframe)导航不应污染整页相位
            Kind::FrameNavigated => {
                if is_main_frame(label, v) {
                    streams::phase_navigation(tab);
                }
            }
            Kind::LoadEvent => {
                streams::phase_load_finished(tab);
                // 持久注入规则(1.12.0):只起后台线程,不在 UI 线程做 eval
                crate::inject::on_page_load(app, label);
            }
            Kind::NetRequest => {
                let Some(request_id) = v.get("requestId").and_then(|r| r.as_str()) else {
                    return;
                };
                let Some(req) = v.get("request") else { return };
                let url = req.get("url").and_then(|u| u.as_str()).unwrap_or("");
                if url.is_empty() {
                    return;
                }
                let method = req
                    .get("method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("GET")
                    .to_string();
                let typ = streams::normalize_network_type(
                    v.get("type").and_then(|t| t.as_str()).unwrap_or("other"),
                );
                // CDP initiator 的脚本栈在 stack.callFrames[0](fetch/xhr 顶层 url 常为空)
                let initiator = streams::initiator_text(v.get("initiator"));
                let headers = headers_from_json(req.get("headers"));
                streams::network_request_started(
                    tab,
                    request_id,
                    streams::PendingRequest {
                        method,
                        url: url.to_string(),
                        typ,
                        initiator,
                        start_ts: streams::now_ms(),
                        headers,
                    },
                );
            }
            Kind::NetResponse => {
                let Some(request_id) = v.get("requestId").and_then(|r| r.as_str()) else {
                    return;
                };
                let Some(resp) = v.get("response") else {
                    return;
                };
                let pending = streams::take_pending_request(tab, request_id);
                let url = resp
                    .get("url")
                    .and_then(|u| u.as_str())
                    .map(str::to_string)
                    .or_else(|| pending.as_ref().map(|p| p.url.clone()))
                    .unwrap_or_default();
                if url.is_empty() {
                    return;
                }
                let status = resp.get("status").and_then(|s| s.as_i64()).unwrap_or(0) as i32;
                let mut headers = pending
                    .as_ref()
                    .map(|p| p.headers.clone())
                    .unwrap_or_default();
                headers.extend(headers_from_json(resp.get("headers")));
                streams::push_network(NetworkEvent {
                    method: pending
                        .as_ref()
                        .map(|p| p.method.clone())
                        .unwrap_or_else(|| "GET".into()),
                    url,
                    status,
                    typ: pending.as_ref().map(|p| p.typ.clone()).unwrap_or_else(|| {
                        streams::normalize_network_type(
                            v.get("type").and_then(|t| t.as_str()).unwrap_or("other"),
                        )
                    }),
                    initiator: pending
                        .as_ref()
                        .map(|p| p.initiator.clone())
                        .unwrap_or_else(|| "other".into()),
                    duration_ms: pending
                        .as_ref()
                        .map(|p| streams::now_ms().saturating_sub(p.start_ts))
                        .unwrap_or(0),
                    failure: None,
                    phase: String::new(),
                    ts: streams::now_ms(),
                    tab,
                    headers,
                });
            }
            Kind::NetFailed => {
                let Some(request_id) = v.get("requestId").and_then(|r| r.as_str()) else {
                    return;
                };
                let pending = streams::take_pending_request(tab, request_id);
                let Some(p) = pending else { return };
                let failure = v
                    .get("errorText")
                    .and_then(|e| e.as_str())
                    .unwrap_or("net::ERR_FAILED")
                    .to_string();
                streams::push_network(NetworkEvent {
                    method: p.method,
                    url: p.url,
                    status: 0,
                    typ: p.typ,
                    initiator: p.initiator,
                    duration_ms: streams::now_ms().saturating_sub(p.start_ts),
                    failure: Some(failure),
                    phase: String::new(),
                    ts: streams::now_ms(),
                    tab,
                    headers: p.headers,
                });
            }
            Kind::NetFinished => {
                if let Some(request_id) = v.get("requestId").and_then(|r| r.as_str()) {
                    streams::drop_pending_request(tab, request_id);
                }
            }
        }
    }

    /// `Runtime.consoleAPICalled.args` → 单行文本
    fn args_to_text(args: Option<&serde_json::Value>) -> String {
        let Some(items) = args.and_then(|a| a.as_array()) else {
            return String::new();
        };
        let mut out: Vec<String> = Vec::with_capacity(items.len());
        for a in items {
            let s = if let Some(v) = a.get("value") {
                match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => "null".into(),
                    other => other.to_string(),
                }
            } else if let Some(u) = a.get("unserializableValue").and_then(|u| u.as_str()) {
                u.to_string()
            } else if let Some(d) = a.get("description").and_then(|d| d.as_str()) {
                d.to_string()
            } else {
                String::new()
            };
            if !s.is_empty() {
                out.push(s);
            }
        }
        out.join(" ")
    }

    /// CDP StackTrace → 可读文本(最多 8 帧)
    fn stack_trace(st: Option<&serde_json::Value>) -> Option<String> {
        let frames = st?.get("callFrames")?.as_array()?;
        if frames.is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        for f in frames.iter().take(8) {
            let func = f
                .get("functionName")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("(anonymous)");
            let url = f.get("url").and_then(|s| s.as_str()).unwrap_or("");
            let line = f.get("lineNumber").and_then(|s| s.as_i64()).unwrap_or(0);
            let col = f.get("columnNumber").and_then(|s| s.as_i64()).unwrap_or(0);
            lines.push(format!("{func} {url}:{line}:{col}"));
        }
        Some(lines.join("\n"))
    }

    /// CDP headers 对象 → Vec(忽略非字符串值)
    fn headers_from_json(v: Option<&serde_json::Value>) -> Vec<(String, String)> {
        let Some(map) = v.and_then(|h| h.as_object()) else {
            return Vec::new();
        };
        map.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::is_main_frame;
        use serde_json::json;

        /// 只有主帧(无 parentId / frameId == 已记录主帧 id)才置 load;iframe 导航不算
        #[test]
        fn only_main_frame_navigation_is_recognized() {
            let label = "test-page-main-frame";
            // 首次:无 parentId → 主帧,记录 id
            assert!(is_main_frame(label, &json!({"frame": {"id": "A"}})));
            // 子帧(有 parentId)→ 不算
            assert!(!is_main_frame(
                label,
                &json!({"frame": {"id": "B", "parentId": "A"}})
            ));
            // 已知主帧再次上报(缺 parentId)→ 算
            assert!(is_main_frame(label, &json!({"frame": {"id": "A"}})));
            // 缺 parentId → 按契约视为主帧(主帧 id 变化时刷新记录)
            assert!(is_main_frame(label, &json!({"frame": {"id": "C"}})));
            // 新主帧下的子帧 → 不算
            assert!(!is_main_frame(
                label,
                &json!({"frame": {"id": "D", "parentId": "C"}})
            ));
            assert!(is_main_frame(label, &json!({"frame": {"id": "C"}})));
            super::release(label);
        }
    }
}

// ---------------------------------------------------------------------------
// Linux:WebKitGTK 资源信号 + load-changed 相位
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod linux_webkit {
    use super::*;

    /// 注册 Linux 页面采集(console 由注入脚本回传,这里只接网络与相位)
    pub(super) fn setup(app: &AppHandle, webview: &tauri::webview::Webview, label: &str) {
        use gtk::prelude::*;
        use webkit2gtk::{LoadEvent, URIRequestExt, URIResponseExt, WebResourceExt, WebViewExt};

        let app = app.clone();
        let label = label.to_string();
        let _ = webview.with_webview(move |platform_webview| {
            let wv = platform_webview.inner();

            let phase_app = app.clone();
            let phase_label = label.clone();
            wv.connect_load_changed(move |_wv, event| {
                let tab = crate::ui::tab_id_for_label(&phase_app, &phase_label).unwrap_or(0);
                match event {
                    LoadEvent::Started => streams::phase_navigation(tab),
                    LoadEvent::Finished => {
                        streams::phase_load_finished(tab);
                        // 持久注入规则(1.12.0):与 Windows 的 Page.loadEventFired 同义
                        crate::inject::on_page_load(&phase_app, &phase_label);
                    }
                    _ => {}
                }
            });

            wv.connect_resource_load_started(move |wv, resource, request| {
                let tab = crate::ui::tab_id_for_label(&app, &label).unwrap_or(0);
                let url = resource
                    .uri()
                    .map(|u| u.to_string())
                    .or_else(|| request.uri().map(|u| u.to_string()))
                    .unwrap_or_default();
                if url.is_empty() {
                    return;
                }
                let method = request
                    .http_method()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "GET".into());
                // 主文档与子资源:与当前 webview URI 相同视为 document
                let main_uri = wv.uri().map(|u| u.to_string()).unwrap_or_default();
                let initiator = if !main_uri.is_empty() && main_uri == url {
                    "document"
                } else {
                    "subresource"
                };
                let start = std::time::Instant::now();
                let pushed = std::rc::Rc::new(std::cell::Cell::new(false));

                let pushed2 = pushed.clone();
                let method2 = method.clone();
                let initiator2 = initiator.to_string();
                let url2 = url.clone();
                resource.connect_notify_local(Some("response"), move |resource, _| {
                    if pushed2.replace(true) {
                        return;
                    }
                    let Some(resp) = resource.response() else {
                        return;
                    };
                    let status = resp.status_code() as i32;
                    let mime = resp.mime_type().map(|m| m.to_string()).unwrap_or_default();
                    streams::push_network(NetworkEvent {
                        method: method2.clone(),
                        url: url2.clone(),
                        status,
                        typ: streams::network_type_from_mime(&mime),
                        initiator: initiator2.clone(),
                        duration_ms: start.elapsed().as_millis() as u64,
                        failure: None,
                        phase: String::new(),
                        ts: streams::now_ms(),
                        tab,
                        headers: Vec::new(),
                    });
                });

                let pushed3 = pushed;
                let initiator3 = initiator.to_string();
                resource.connect_failed(move |_resource, err| {
                    if pushed3.replace(true) {
                        return;
                    }
                    streams::push_network(NetworkEvent {
                        method: method.clone(),
                        url: url.clone(),
                        status: 0,
                        typ: "other".into(),
                        initiator: initiator3.clone(),
                        duration_ms: start.elapsed().as_millis() as u64,
                        failure: Some(err.message().to_string()),
                        phase: String::new(),
                        ts: streams::now_ms(),
                        tab,
                        headers: Vec::new(),
                    });
                });
            });
        });
    }
}
