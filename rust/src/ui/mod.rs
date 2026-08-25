//! Tauri UI 模块:单窗口 + 三 Webview(页面/覆盖层/面板)
//! 页面 Webview 注入 page-bridge(零注入,无残留脚本)
//! 覆盖层 Webview 透明叠加,面板 Webview 右侧并排
pub mod annotate;

use std::sync::Mutex;

use tauri::window::WindowBuilder;
use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, Manager, WebviewUrl};

/// 页面 Webview 标签
pub const PAGE_WEBVIEW: &str = "page";
/// 覆盖层 Webview 标签
pub const OVERLAY_WEBVIEW: &str = "overlay";
/// 面板 Webview 标签
pub const PANEL_WEBVIEW: &str = "panel";

/// 面板宽度(逻辑像素)
pub const PANEL_WIDTH: f64 = 280.0;

/// 批注记录(Rust 侧持有,推送面板)
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnnotationRecord {
    pub index: u32,
    pub selector: String,
    pub rect: (i32, i32, i32, i32),
    pub note: String,
}

/// UI 状态(跨线程共享)
pub struct UiState {
    /// 批注模式开关
    pub annotate_mode: Mutex<bool>,
    /// 批注记录
    pub records: Mutex<Vec<AnnotationRecord>>,
    /// 待发送记录队列(面板点击"发送全部"后,插件端轮询消费)
    pub sent_records: Mutex<Vec<serde_json::Value>>,
    /// 开发者工具开关(wry 的 is_devtools_open 在 webview2 上恒 false,需自行维护)
    pub devtools_open: Mutex<bool>,
    /// HTTP 服务端口(面板 invoke 获取后 fetch /api/*)
    pub service_port: Mutex<u16>,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            annotate_mode: Mutex::new(false),
            records: Mutex::new(Vec::new()),
            sent_records: Mutex::new(Vec::new()),
            devtools_open: Mutex::new(false),
            service_port: Mutex::new(0),
        }
    }
}

/// 页面桥脚本(注入页面 Webview,提供 elementFromPoint/selector 等工具)
const PAGE_BRIDGE_JS: &str = r##"
(function(){
  function escapeCss(s){return s.replace(/([^a-zA-Z0-9_-])/g,"\\$1");}
  function buildSelector(node){
    var parts=[],cur=node;
    while(cur&&cur.nodeType===1&&cur!==document.documentElement){
      var tag=cur.tagName.toLowerCase(),part=tag;
      if(cur.id){part+="#"+escapeCss(cur.id);parts.unshift(part);break;}
      var cls=Array.prototype.slice.call(cur.classList,0,2).map(escapeCss);
      if(cls.length)part+="."+cls.join(".");
      var parent=cur.parentElement;
      if(parent){
        var sibs=Array.prototype.slice.call(parent.children);
        var same=sibs.filter(function(c){return c.tagName===cur.tagName;});
        if(same.length>1)part+=":nth-child("+(sibs.indexOf(cur)+1)+")";
      }
      parts.unshift(part);cur=parent;
    }
    return parts.join(" > ");
  }
  window.__btPage={
    query:function(x,y){
      var el=document.elementFromPoint(x,y);
      if(!el||el===document.documentElement||el===document.body)return null;
      var r=el.getBoundingClientRect();
      return {selector:buildSelector(el),rect:{x:r.x,y:r.y,w:r.width,h:r.height},tag:el.tagName.toLowerCase()};
    },
    state:function(){
      return {url:location.href,title:document.title,readyState:document.readyState,
        viewport:{w:innerWidth,h:innerHeight,dpr:devicePixelRatio}};
    }
  };
  // 控制台日志捕获:劫持 console.* 存入 __btLogs(service /api/console-logs 读取)
  window.__btLogs=[];
  ["log","info","warn","error","debug"].forEach(function(level){
    var orig=console[level]&&console[level].bind(console);
    console[level]=function(){
      var msg=Array.prototype.map.call(arguments,function(a){
        try{return (typeof a==="object"&&a!==null)?JSON.stringify(a):String(a);}catch(e){return String(a);}
      }).join(" ");
      if(window.__btLogs.length>=500)window.__btLogs.shift();
      window.__btLogs.push({level:level,msg:msg});
      if(orig)orig.apply(null,arguments);
    };
  });
})();
"##;

/// 创建主窗口 + 三 Webview
/// 布局以窗口实际物理尺寸为准(避免 DPI 感知时序导致窗口与子 webview 缩放不一致)
pub fn create_ui(app: &AppHandle) -> tauri::Result<()> {
    // 主窗口(逻辑 1100x700:150% DPI 下物理 1650x1050,适配常见 1920x1080 屏幕)
    let window = WindowBuilder::new(app, "main")
        .title("bt-shell")
        .inner_size(1100.0, 700.0)
        .build()?;

    // 1. 页面 Webview(左侧,渲染目标网页 + 注入页面桥)
    window.add_child(
        WebviewBuilder::new(
            PAGE_WEBVIEW,
            WebviewUrl::External("about:blank".parse().unwrap()),
        )
        .initialization_script(PAGE_BRIDGE_JS),
        tauri::PhysicalPosition::new(0, 0),
        tauri::PhysicalSize::new(100, 100),
    )?;

    // 2. 覆盖层 Webview(透明,叠加在页面区)
    window.add_child(
        WebviewBuilder::new(OVERLAY_WEBVIEW, WebviewUrl::App("overlay.html".into()))
            .transparent(true)
            .disable_drag_drop_handler(),
        tauri::PhysicalPosition::new(0, 0),
        tauri::PhysicalSize::new(100, 100),
    )?;

    // 3. 面板 Webview(右侧并排)
    window.add_child(
        WebviewBuilder::new(PANEL_WEBVIEW, WebviewUrl::App("index.html".into())),
        tauri::PhysicalPosition::new(100, 0),
        tauri::PhysicalSize::new(100, 100),
    )?;

    // 初始布局按窗口实际尺寸重排
    let size = window.inner_size()?;
    let scale = window.scale_factor().unwrap_or(1.0);
    apply_layout(app, size, scale)?;

    // 监听窗口 resize/DPI 变化 → 重排三个 webview
    let handle = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(size) | tauri::WindowEvent::ScaleFactorChanged { new_inner_size: size, .. } => {
            if let Some(win) = handle.get_window("main") {
                let scale = win.scale_factor().unwrap_or(1.0);
                let _ = apply_layout(&handle, *size, scale);
            }
        }
        _ => {}
    });

    Ok(())
}

/// 重排三个 webview 的边界(页面/覆盖层占左侧,面板占右侧固定宽度)
/// size 为窗口物理内尺寸,scale 为窗口缩放因子
fn apply_layout(app: &AppHandle, size: tauri::PhysicalSize<u32>, scale: f64) -> tauri::Result<()> {
    let panel_w = (PANEL_WIDTH * scale) as i32;
    let page_w = size.width as i32 - panel_w;
    let page_h = size.height as i32;

    // 1. 页面 Webview(左侧,渲染目标网页 + 注入页面桥)
    if let Some(w) = app.get_webview(PAGE_WEBVIEW) {
        w.set_position(tauri::PhysicalPosition::new(0, 0))?;
        w.set_size(tauri::PhysicalSize::new(page_w.max(0) as u32, page_h.max(0) as u32))?;
    }

    // 2. 覆盖层 Webview(透明,叠加在页面区)
    if let Some(w) = app.get_webview(OVERLAY_WEBVIEW) {
        w.set_position(tauri::PhysicalPosition::new(0, 0))?;
        w.set_size(tauri::PhysicalSize::new(page_w.max(0) as u32, page_h.max(0) as u32))?;
    }

    // 3. 面板 Webview(右侧并排)
    if let Some(w) = app.get_webview(PANEL_WEBVIEW) {
        w.set_position(tauri::PhysicalPosition::new(page_w.max(0), 0))?;
        w.set_size(tauri::PhysicalSize::new(panel_w as u32, page_h.max(0) as u32))?;
    }
    Ok(())
}

/// 获取页面 Webview
pub fn page_webview(app: &AppHandle) -> Option<tauri::webview::Webview> {
    app.get_webview(PAGE_WEBVIEW)
}

/// 获取覆盖层 Webview
pub fn overlay_webview(app: &AppHandle) -> Option<tauri::webview::Webview> {
    app.get_webview(OVERLAY_WEBVIEW)
}

/// 获取面板 Webview
pub fn panel_webview(app: &AppHandle) -> Option<tauri::webview::Webview> {
    app.get_webview(PANEL_WEBVIEW)
}

/// 在页面 Webview 执行 JS,返回 JSON 结果(控制协议核心)
pub fn eval_page(app: &AppHandle, js: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let page = page_webview(app).ok_or("page webview not ready")?;
    page.eval_with_callback(
        js,
        move |result| {
            let _ = tx.send(Ok(result));
        },
    )
    .map_err(|e| format!("eval failed: {e}"))?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "eval timeout".to_string())?
}

/// 在覆盖层 Webview 执行 JS
pub fn eval_overlay(app: &AppHandle, js: &str) -> Result<(), String> {
    let overlay = overlay_webview(app).ok_or("overlay webview not ready")?;
    overlay.eval(js).map_err(|e| format!("overlay eval failed: {e}"))
}
