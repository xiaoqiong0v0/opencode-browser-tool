//! 可访问性树:返回 {role,name,value,children} 嵌套结构
//! Windows:CDP Accessibility.getFullAXTree(复用截图同款 ICoreWebView2 + CallDevToolsProtocolMethod 通道)
//! Linux(WebKitGTK):ATK/AT-SPI 需要可用的 a11y bus(WSLg/Kali 不可用),改用页面 DOM 近似重建(同结构)
#[cfg(windows)]
use std::sync::mpsc;

use tauri::AppHandle;

#[cfg(any(windows, target_os = "linux"))]
use crate::ui;
#[cfg(windows)]
use serde_json::json;

/// 获取可访问性树,返回 {role,name,value,children} 嵌套结构
/// selector 为 None/空:整页(行为不变);指定时返回该选择器对应元素的子树
pub fn accessibility_tree(
    app: &AppHandle,
    selector: Option<&str>,
) -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    {
        match selector {
            // 有 selector:走 DOM 节点定位 + 部分 AX 树
            Some(s) if !s.is_empty() => cdp_subtree(app, s),
            // 无 selector:整页(原行为)
            _ => {
                let raw = cdp_call_json(app, "Accessibility.getFullAXTree", "{}")?;
                // CDP 返回 { "nodes": [...] }
                let v: serde_json::Value =
                    serde_json::from_str(&raw).map_err(|e| format!("parse cdp result: {e}"))?;
                let nodes = v.get("nodes").and_then(|n| n.as_array()).cloned().unwrap_or_default();
                build_tree(&nodes)
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        dom_accessibility_tree(app, selector)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (app, selector);
        Err("accessibility not implemented on this platform yet".into())
    }
}

/// 调用任意 CDP 方法并返回其结果 JSON
/// jsonResult 即 CDP 的 result 对象本身(不带 id/result 信封)
#[cfg(windows)]
fn cdp_call_json(app: &AppHandle, method_name: &str, params_json: &str) -> Result<String, String> {
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let tx2 = tx.clone();
    let method_str = method_name.to_string();
    let params_str = params_json.to_string();
    page.with_webview(move |platform_webview| {
        let controller = platform_webview.controller();
        unsafe {
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
            use windows::core::Interface;

            let webview_result: Result<ICoreWebView2, String> = controller
                .CoreWebView2()
                .map_err(|e| format!("get webview failed: {e}"));
            match webview_result {
                Err(e) => { let _ = tx.send(Err(e)); }
                Ok(webview) => {
                    let method = windows::core::HSTRING::from(method_str.as_str());
                    let params = windows::core::HSTRING::from(params_str.as_str());
                    let result = CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
                        Box::new(move |handler| {
                            webview
                                .CallDevToolsProtocolMethod(&method, &params, &handler)
                                .map_err(webview2_com::Error::from)
                        }),
                        Box::new(
                            move |_result: windows::core::Result<()>, json: String| -> windows::core::Result<()> {
                                let _ = tx2.send(Ok(json));
                                Ok(())
                            },
                        ),
                    );
                    if let Err(e) = result {
                        let _ = tx.send(Err(e.to_string()));
                    }
                }
            }
        }
    })
    .map_err(|e| format!("with_webview failed: {e}"))?;

    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "accessibility timeout".to_string())?
}

/// 按 selector 取元素子树(Windows)
/// 链路:DOM.getDocument → DOM.querySelector 拿 nodeId → Accessibility.getPartialAXTree
/// fetchRelatives=false 只取该节点自身+子树,与 Linux 子树语义一致
#[cfg(windows)]
fn cdp_subtree(app: &AppHandle, selector: &str) -> Result<serde_json::Value, String> {
    // 1. 取文档根节点(depth:0 只需 root)
    let doc_raw = cdp_call_json(app, "DOM.getDocument", r#"{"depth":0}"#)?;
    let doc: serde_json::Value =
        serde_json::from_str(&doc_raw).map_err(|e| format!("parse DOM.getDocument: {e}"))?;
    let root_id = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(|v| v.as_i64())
        .ok_or("DOM.getDocument: no root nodeId")?;
    // 2. 查询选择器命中的节点(nodeId=0 表示未命中)
    let q_params = json!({ "nodeId": root_id, "selector": selector }).to_string();
    let q_raw = cdp_call_json(app, "DOM.querySelector", &q_params)?;
    let q: serde_json::Value =
        serde_json::from_str(&q_raw).map_err(|e| format!("parse DOM.querySelector: {e}"))?;
    let node_id = q.get("nodeId").and_then(|v| v.as_i64()).unwrap_or(0);
    if node_id == 0 {
        return Err(format!("element not found: {selector}"));
    }
    // 3. 该节点自身+子树的部分 AX 树(fetchRelatives=false 排除祖先/兄弟)
    let p_params = json!({ "nodeId": node_id, "fetchRelatives": false }).to_string();
    let p_raw = cdp_call_json(app, "Accessibility.getPartialAXTree", &p_params)?;
    let p: serde_json::Value =
        serde_json::from_str(&p_raw).map_err(|e| format!("parse getPartialAXTree: {e}"))?;
    let nodes = p.get("nodes").and_then(|n| n.as_array()).cloned().unwrap_or_default();
    build_tree(&nodes)
}

/// 页面 DOM → 近似可访问性树(Linux/WebKitGTK)
/// 用页面 JS 重建同结构树;selector 为 None/空从整页根构建,指定时从该元素起构建
/// 解析失败/页面无内容时返回与 Windows 一致的错误
#[cfg(target_os = "linux")]
fn dom_accessibility_tree(
    app: &AppHandle,
    selector: Option<&str>,
) -> Result<serde_json::Value, String> {
    // selector 以 JSON 字面量注入 JS(避免手工转义),未指定时为 null
    let sel_lit = match selector {
        Some(s) if !s.is_empty() => serde_json::to_string(s).unwrap_or_else(|_| "null".to_string()),
        _ => "null".to_string(),
    };
    let js = DOM_AX_JS.replace("__BT_AX_SELECTOR__", &sel_lit);
    let raw = ui::eval_page(app, &js)?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse dom ax result: {e}"))?;
    // 选择器未命中:JS 返回 {error},直接透传为业务错误
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(err.to_string());
    }
    // JS 失败/无节点时返回 null;根节点必须带 role
    let has_role = v
        .as_object()
        .and_then(|o| o.get("role"))
        .and_then(|r| r.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if !has_role {
        return Err("no accessibility nodes".into());
    }
    Ok(v)
}

/// 页面 DOM → 近似可访问性树(内嵌 JS,与 Windows CDP 产物同结构)
/// 结构:{role,name,value,children};过滤策略与 build_tree 一致(role 空 / generic 无 name 时提升子树)
/// 占位符 `__BT_AX_SELECTOR__` 由 Rust 侧替换为 JSON 字符串或 null(限定子树根)
#[cfg(target_os = "linux")]
const DOM_AX_JS: &str = r##"
(function(){
  try{
    // 递归深度上限:防御异常深的 DOM
    var MAX_DEPTH=100;
    // 非内容元素:不参与可访问性树
    var SKIP_TAGS={SCRIPT:1,STYLE:1,NOSCRIPT:1,TEMPLATE:1,HEAD:1,META:1,LINK:1,TITLE:1,BASE:1};
    // 元素可见性:不可见元素及其子树跳过
    function isVisible(el){
      var tag=el.tagName?el.tagName.toUpperCase():"";
      // body/html 的 offsetParent 恒为 null,不能据此判为隐藏
      if(tag==="BODY"||tag==="HTML")return true;
      if(el.getClientRects&&el.getClientRects().length===0)return false;
      if(el.offsetParent===null){
        var pos="";
        try{pos=getComputedStyle(el).position;}catch(e){}
        // position:fixed 的 offsetParent 恒为 null,不能据此判为隐藏
        if(pos!=="fixed")return false;
      }
      return true;
    }
    // aria-labelledby:解析被引用元素的文本,拼接为可访问名
    function labelledBy(el){
      var ids=(el.getAttribute("aria-labelledby")||"").trim();
      if(!ids)return "";
      var parts=[],list=ids.split(/\s+/);
      for(var i=0;i<list.length;i++){
        var ref=document.getElementById(list[i]);
        if(!ref)continue;
        var t=(ref.innerText||ref.textContent||"").replace(/\s+/g," ").trim();
        if(t)parts.push(t);
      }
      return parts.join(" ").trim();
    }
    // 可访问名:aria-label → aria-labelledby → img alt → title → placeholder → 自身可见文本(≤120 字符)
    function nameOf(el){
      var tag=el.tagName?el.tagName.toUpperCase():"";
      var v=el.getAttribute("aria-label");
      if(v&&v.trim())return v.trim();
      v=labelledBy(el);
      if(v)return v;
      if(tag==="IMG"){
        var alt=el.getAttribute("alt");
        if(alt&&alt.trim())return alt.trim();
      }
      v=el.getAttribute("title");
      if(v&&v.trim())return v.trim();
      v=el.getAttribute("placeholder");
      if(v&&v.trim())return v.trim();
      // 结构性容器不用自身文本,避免整页文本成为根名
      if(tag==="BODY"||tag==="HTML")return "";
      var t=(el.innerText||"").replace(/\s+/g," ").trim();
      if(t.length>120)t=t.slice(0,120);
      return t;
    }
    // 显式 role 属性(非空才有效)
    function explicitRole(el){
      var r=el.getAttribute("role");
      return r&&r.trim()?r.trim().toLowerCase():"";
    }
    // 隐式 role 映射(按标签/type)
    function implicitRole(el){
      var tag=el.tagName?el.tagName.toLowerCase():"";
      var type=(el.getAttribute("type")||"").toLowerCase();
      switch(tag){
        case "a":
        case "area":
          return el.hasAttribute("href")?"link":"";
        case "button":return "button";
        case "input":
          if(type==="checkbox")return "checkbox";
          if(type==="radio")return "radio";
          if(type==="range")return "slider";
          if(type==="number")return "spinbutton";
          if(type==="search")return "searchbox";
          if(type==="button"||type==="submit"||type==="reset"||type==="image")return "button";
          if(type==="hidden")return "";
          return "textbox";
        case "select":return "combobox";
        case "textarea":return "textbox";
        case "h1":case "h2":case "h3":case "h4":case "h5":case "h6":return "heading";
        case "img":return "img";
        case "ul":case "ol":return "list";
        case "li":return "listitem";
        case "table":return "table";
        case "nav":return "navigation";
        case "form":return "form";
        case "main":return "main";
        case "header":return "banner";
        case "footer":return "contentinfo";
        case "dialog":return "dialog";
        case "progress":return "progressbar";
        default:return "";
      }
    }
    // 值:复选状态 / 表单值 / aria-* 状态(统一转字符串)
    function valueOf(el,role){
      if(role==="checkbox"||role==="radio"){
        var ac=el.getAttribute("aria-checked");
        if(ac!==null&&ac!=="")return ac;
        if(typeof el.checked==="boolean")return el.checked?"true":"false";
      }
      var tag=el.tagName?el.tagName.toUpperCase():"";
      if(tag==="INPUT"||tag==="TEXTAREA"||tag==="SELECT"){
        var v=el.value;
        if(v!==undefined&&v!==null&&String(v)!=="")return String(v);
      }
      var attrs=["aria-valuenow","aria-checked","aria-pressed","aria-expanded","aria-selected"];
      for(var i=0;i<attrs.length;i++){
        var a=el.getAttribute(attrs[i]);
        if(a!==null&&a!=="")return a;
      }
      return "";
    }
    // 递归构建:返回输出节点数组(被过滤节点自身不输出,子树提升到父级)
    function walk(el,depth){
      var out=[];
      if(depth>MAX_DEPTH)return out;
      var tag=el.tagName?el.tagName.toUpperCase():"";
      if(SKIP_TAGS[tag])return out;
      if(!isVisible(el))return out;
      // 子节点(保持 DOM 顺序)
      var children=[],kids=el.children||[];
      for(var i=0;i<kids.length;i++){
        var sub=walk(kids[i],depth+1);
        for(var j=0;j<sub.length;j++)children.push(sub[j]);
      }
      var role=explicitRole(el)||implicitRole(el);
      var name=nameOf(el);
      // 无 role 但有文本的容器归为 generic
      if(!role&&name)role="generic";
      // 与 Windows build_tree 一致:role 为空或 generic 无 name 时自身不输出,子树提升
      if(!role||(role==="generic"&&!name))return children;
      var node={role:role};
      if(name)node.name=name;
      var val=valueOf(el,role);
      if(val!=="")node.value=val;
      if(children.length)node.children=children;
      return [node];
    }
    var sel=__BT_AX_SELECTOR__;
    var root;
    if(sel){
      root=document.querySelector(sel);
      if(!root)return {error:"element not found"};
    }else{
      root=document.body||document.documentElement;
    }
    if(!root)return null;
    var nodes=walk(root,0);
    if(!nodes.length)return null;
    if(nodes.length===1)return nodes[0];
    // 顶层被提升为多个节点时合成虚拟根
    return {role:"generic",children:nodes};
  }catch(e){
    return null;
  }
})()
"##;

/// CDP AX nodes → {role,name,value,children} 树(过滤 ignored/backdrop/none role)
#[cfg(windows)]
fn build_tree(nodes: &[serde_json::Value]) -> Result<serde_json::Value, String> {
    // 构建 children 映射:parentId → [nodeId]
    use std::collections::HashMap;
    let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut id_node: HashMap<String, &serde_json::Value> = HashMap::new();
    for n in nodes {
        let id = n.get("nodeId").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let parent = n.get("parentId").and_then(|v| v.as_str()).unwrap_or("");
        if !parent.is_empty() {
            children_map.entry(parent.to_string()).or_default().push(id.to_string());
        }
        id_node.insert(id.to_string(), n);
    }

    fn convert(
        id: &str,
        id_node: &HashMap<String, &serde_json::Value>,
        children_map: &HashMap<String, Vec<String>>,
    ) -> Vec<serde_json::Value> {
        let Some(n) = id_node.get(id) else { return Vec::new() };
        // 过滤忽略/遮挡节点
        let role = n.get("role").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        let skip = n.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false)
            || role.is_empty()
            || (role == "generic" && n.get("name").is_none());
        // 子节点(保持 DOM 顺序),被过滤的节点自身不输出但其子树提升到父级
        let mut children_out = Vec::new();
        if let Some(ids) = children_map.get(id) {
            for cid in ids {
                children_out.extend(convert(cid, id_node, children_map));
            }
        }
        if skip {
            return children_out;
        }
        let mut node = serde_json::Map::new();
        node.insert("role".into(), json!(role));
        if let Some(v) = n.get("name").and_then(|m| m.get("value")).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                node.insert("name".into(), json!(v));
            }
        }
        // value 可能是字符串或其它(如数字),统一转字符串
        let raw = n.get("value").and_then(|m| m.get("value"));
        if let Some(v) = raw {
            match v {
                serde_json::Value::String(s) if !s.is_empty() => {
                    node.insert("value".into(), json!(s));
                }
                serde_json::Value::Number(num) => {
                    node.insert("value".into(), json!(num.as_i64().unwrap_or(0).to_string()));
                }
                serde_json::Value::Bool(b) => {
                    node.insert("value".into(), json!(b.to_string()));
                }
                _ => {}
            }
        }
        let mut obj = serde_json::Value::Object(node);
        if !children_out.is_empty() {
            obj.as_object_mut().unwrap().insert("children".into(), serde_json::Value::Array(children_out));
        }
        vec![obj]
    }

    // 根:parentId 为空的节点(可能存在多个,取第一个有输出的)
    let mut roots = Vec::new();
    for n in nodes {
        let id = n.get("nodeId").and_then(|v| v.as_str()).unwrap_or("");
        let has_parent = n.get("parentId").map(|p| p.is_string() && !p.as_str().unwrap_or("").is_empty()).unwrap_or(false);
        if !has_parent {
            roots.push(id);
        }
    }
    for r in roots {
        let out = convert(r, &id_node, &children_map);
        if let Some(v) = out.into_iter().next() {
            return Ok(v);
        }
    }
    Err("no accessibility nodes".into())
}