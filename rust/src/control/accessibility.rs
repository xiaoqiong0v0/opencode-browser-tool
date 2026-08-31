//! 可访问性树:通过 CDP Accessibility.getFullAXTree 获取页面可访问性树
//! 复用截图同款通道(ICoreWebView2 + CallDevToolsProtocolMethod)
use std::sync::mpsc;

use serde_json::json;
use tauri::AppHandle;

use crate::ui;

/// 获取可访问性树,返回 {role,name,value,children} 嵌套结构
pub fn accessibility_tree(app: &AppHandle) -> Result<serde_json::Value, String> {
    let raw = cdp_json(app)?;
    // CDP 返回 { "nodes": [...] }
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse cdp result: {e}"))?;
    let nodes = v.get("nodes").and_then(|n| n.as_array()).cloned().unwrap_or_default();
    build_tree(&nodes)
}

#[cfg(windows)]
fn cdp_json(app: &AppHandle) -> Result<String, String> {
    let page = ui::active_page_webview(app).ok_or("page webview not ready")?;
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let tx2 = tx.clone();
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
                    let method = windows::core::HSTRING::from("Accessibility.getFullAXTree");
                    let params = windows::core::HSTRING::from(r#"{}"#);
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

#[cfg(not(windows))]
fn cdp_json(_app: &AppHandle) -> Result<String, String> {
    Err("accessibility not implemented on this platform yet".into())
}

/// CDP AX nodes → {role,name,value,children} 树(过滤 ignored/backdrop/none role)
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