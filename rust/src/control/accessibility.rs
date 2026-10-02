//! 可访问性树:返回 {role,name,value,children} 嵌套结构
//! Windows:CDP(复用截图同款 ICoreWebView2 + CallDevToolsProtocolMethod 通道)
//!   整页与 selector 子树均基于 Accessibility.getFullAXTree(selector 子树另经 DOM 定位命中节点)
//! Linux(WebKitGTK):ATK/AT-SPI 需要可用的 a11y bus(WSLg/Kali 不可用),改用页面 DOM 近似重建(同结构)
use tauri::AppHandle;

#[cfg(windows)]
use crate::control::cdp;
#[cfg(target_os = "linux")]
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
                let raw = cdp::call_json(app, "Accessibility.getFullAXTree", "{}")?;
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

/// 按 selector 取元素子树(Windows)
/// 链路:DOM.getDocument → DOM.querySelector → DOM.describeNode 取 backendNodeId
///      → Accessibility.getFullAXTree → 以命中节点为根重建该元素子树
///
/// 为什么不用 Accessibility.getPartialAXTree:实测 fetchRelatives=false 只返回命中节点
/// 自身(不含任何后代),且返回节点仍带 parentId(祖先不在结果里),build_tree 找不到根 →
/// "no accessibility nodes";即便 fetchRelatives=true 也只多一层直接子节点与祖先,拿不到完整子树。
/// getFullAXTree 是含 ignored 节点的完整快照,配合显式根即可精确取到该元素的子树。
/// Chromium 会把 html/body、无 accessible name 的 generic 容器标为 ignored;
/// ignored 节点交由 build_tree 过滤并提升其子树,与整页语义一致。
#[cfg(windows)]
fn cdp_subtree(app: &AppHandle, selector: &str) -> Result<serde_json::Value, String> {
    // 1. 取文档根节点(depth:0 只需 root)
    let doc_raw = cdp::call_json(app, "DOM.getDocument", r#"{"depth":0}"#)?;
    let doc: serde_json::Value =
        serde_json::from_str(&doc_raw).map_err(|e| format!("parse DOM.getDocument: {e}"))?;
    let root_id = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(|v| v.as_i64())
        .ok_or("DOM.getDocument: no root nodeId")?;
    // 2. 查询选择器命中的 DOM 节点(nodeId=0 表示未命中)
    let q_params = json!({ "nodeId": root_id, "selector": selector }).to_string();
    let q_raw = cdp::call_json(app, "DOM.querySelector", &q_params)?;
    let q: serde_json::Value =
        serde_json::from_str(&q_raw).map_err(|e| format!("parse DOM.querySelector: {e}"))?;
    let node_id = q.get("nodeId").and_then(|v| v.as_i64()).unwrap_or(0);
    if node_id == 0 {
        return Err(format!("element not found: {selector}"));
    }
    // 3. 该节点的 backendNodeId(AX 节点用它定位命中的 DOM 节点)
    let d_params = json!({ "nodeId": node_id }).to_string();
    let d_raw = cdp::call_json(app, "DOM.describeNode", &d_params)?;
    let d: serde_json::Value =
        serde_json::from_str(&d_raw).map_err(|e| format!("parse DOM.describeNode: {e}"))?;
    let backend_id = d
        .get("node")
        .and_then(|n| n.get("backendNodeId"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    // 4. 全量 AX 树快照(含 ignored 节点,父子关系完整)
    //    不用 getPartialAXTree:实测它只返回命中节点 + 直接子节点 + 祖先,
    //    拿不到完整后代子树(且 fetchRelatives=false 时连后代都没有)
    let full_raw = cdp::call_json(app, "Accessibility.getFullAXTree", "{}")?;
    let full: serde_json::Value =
        serde_json::from_str(&full_raw).map_err(|e| format!("parse getFullAXTree: {e}"))?;
    let nodes = full.get("nodes").and_then(|n| n.as_array()).cloned().unwrap_or_default();
    if nodes.is_empty() {
        return Err("no accessibility nodes".into());
    }
    // 5. 在整棵树里定位命中 DOM 节点对应的 AX 节点,显式作为根,只保留其子树
    let target = pick_target_ax_node(&nodes, backend_id)
        .ok_or_else(|| "no accessibility nodes".to_string())?;
    build_tree_from(&nodes, Some(&target))
}

/// 在 AX 节点集合里选出"命中 DOM 节点"对应的 AX 节点 id
/// 按 backendNodeId 匹配;若同一 DOM 节点有多个 AX 节点,优先非 ignored 的那个
#[cfg(windows)]
fn pick_target_ax_node(nodes: &[serde_json::Value], backend_id: i64) -> Option<String> {
    if backend_id == 0 {
        return None;
    }
    let mut ignored_match: Option<String> = None;
    for n in nodes {
        let Some(id) = n.get("nodeId").and_then(|v| v.as_str()) else {
            continue;
        };
        if n.get("backendDOMNodeId").and_then(|v| v.as_i64()) == Some(backend_id) {
            let ignored = n.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false);
            if !ignored {
                return Some(id.to_string());
            }
            if ignored_match.is_none() {
                ignored_match = Some(id.to_string());
            }
        }
    }
    ignored_match
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
/// 根为 parentId 为空的节点(整页树入口)
#[cfg(windows)]
fn build_tree(nodes: &[serde_json::Value]) -> Result<serde_json::Value, String> {
    build_tree_from(nodes, None)
}

/// CDP AX nodes → {role,name,value,children} 树,可显式指定根 AX 节点
/// root_id=None:根为 parentId 为空的节点(整页);Some:以该节点为根(selector 子树,
/// 其 parentId 指向未包含的祖先,必须显式指定,否则找不到根)
#[cfg(windows)]
fn build_tree_from(
    nodes: &[serde_json::Value],
    root_id: Option<&str>,
) -> Result<serde_json::Value, String> {
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

    /// 读取 AX 节点的 name 值:CDP 的 name 对象总是存在(值为空串或缺失),
    /// 因此不能用 name.is_none() 判断"没有名字"
    fn node_name(n: &serde_json::Value) -> &str {
        n.get("name").and_then(|m| m.get("value")).and_then(|v| v.as_str()).unwrap_or("")
    }

    /// 拼接文本上限(与 Linux 侧 DOM_AX_JS 的 120 字符对齐)
    const TEXT_MAX_LEN: usize = 120;

    fn convert(
        id: &str,
        id_node: &HashMap<String, &serde_json::Value>,
        children_map: &HashMap<String, Vec<String>>,
    ) -> Vec<serde_json::Value> {
        let Some(n) = id_node.get(id) else { return Vec::new() };
        let role = n.get("role").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        // 先转换子节点(被过滤的节点自身不输出、其子树提升到当前层)
        let mut child_nodes: Vec<serde_json::Value> = Vec::new();
        if let Some(ids) = children_map.get(id) {
            for cid in ids {
                child_nodes.extend(convert(cid, id_node, children_map));
            }
        }
        // CDP 会把一段文本拆成多个(甚至逐字符的)StaticText 子节点:
        // 按顺序拼接其文本;本节点自身无 name 时以拼接结果(截断)为 name,
        // 已有 name 则保留不覆盖;这些 StaticText 不再单独输出(避免重复)
        let mut merged = String::new();
        let mut kept: Vec<serde_json::Value> = Vec::with_capacity(child_nodes.len());
        let mut statics: Vec<serde_json::Value> = Vec::new();
        for cn in child_nodes {
            if cn.get("role").and_then(|r| r.as_str()) == Some("StaticText") {
                if let Some(t) = cn.get("name").and_then(|v| v.as_str()) {
                    merged.push_str(t);
                }
                statics.push(cn);
            } else {
                kept.push(cn);
            }
        }
        let own = node_name(n);
        let name = if !own.is_empty() {
            own.to_string()
        } else {
            merged.trim().chars().take(TEXT_MAX_LEN).collect::<String>()
        };
        // 过滤:ignored / 空 role / 纯排版叶子 InlineTextBox / 无名 generic(按 name 值判断)
        let skip = n.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false)
            || role.is_empty()
            || role == "InlineTextBox"
            || (role == "generic" && name.is_empty());
        if skip {
            // 未输出的节点不吞文本:把 StaticText 放回,让文本冒泡到最近的可输出祖先再合并
            kept.extend(statics);
            return kept;
        }
        let mut node = serde_json::Map::new();
        node.insert("role".into(), json!(role));
        if !name.is_empty() {
            node.insert("name".into(), json!(name));
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
        if !kept.is_empty() {
            obj.as_object_mut().unwrap().insert("children".into(), serde_json::Value::Array(kept));
        }
        vec![obj]
    }

    // 选根:显式指定则以其为根;否则取 parentId 为空的节点
    let mut roots: Vec<String> = Vec::new();
    match root_id {
        Some(r) => {
            if id_node.contains_key(r) {
                roots.push(r.to_string());
            }
        }
        None => {
            for n in nodes {
                let id = n.get("nodeId").and_then(|v| v.as_str()).unwrap_or("");
                let has_parent = n
                    .get("parentId")
                    .map(|p| p.is_string() && !p.as_str().unwrap_or("").is_empty())
                    .unwrap_or(false);
                if !has_parent {
                    roots.push(id.to_string());
                }
            }
        }
    }
    for r in roots {
        let out = convert(&r, &id_node, &children_map);
        match out.len() {
            0 => continue,
            // 根被过滤且有多个被提升的孩子时,合成虚拟根(与 Linux 端一致)
            1 => return Ok(out.into_iter().next().unwrap()),
            _ => return Ok(json!({ "role": "generic", "children": out })),
        }
    }
    Err("no accessibility nodes".into())
}