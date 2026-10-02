//! 文件上传:把本机文件挂到页面 `<input type="file">`
//!
//! Windows:CDP `DOM.setFileInputFiles`(可信通道,直接把本机绝对路径挂到 input.files)
//! Linux/其他:未实现,返回明确错误
//!   (旧 `fetch("file:///...")` 方案在非 file:// 源页面被 scheme/CORS 拦截,且异步未等待 → 必然假成功)
//!
//! 所有路径先做存在性校验;Windows 注入后回读 `el.files` 校验真的挂上,失败不报成功。

use tauri::AppHandle;

use super::ControlResult;

#[cfg(windows)]
use serde_json::{json, Value};
#[cfg(windows)]
use super::cdp;

/// 上传本机文件到 `selector` 指向的 file input
///
/// 参数:`app` 应用句柄;`selector` file input 选择器;`file_path` 本机文件路径(相对按进程 cwd 解析)
/// 返回:成功 Ok(());文件不存在 / 元素不存在 / 非 file input / 校验失败 / 平台未实现均返回 Err
pub fn set_file(app: &AppHandle, selector: &str, file_path: &str) -> ControlResult<()> {
    // 存在性校验(平台无关,先于任何注入)
    let path = std::path::Path::new(file_path);
    if !path.is_file() {
        return Err(format!("file not found: {file_path}"));
    }

    #[cfg(windows)]
    {
        set_file_windows(app, selector, file_path)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, selector);
        Err("upload_file: not implemented on this platform yet \
             (Windows via CDP DOM.setFileInputFiles; Linux/WebKitGTK unverified)"
            .into())
    }
}

/// Windows:CDP `DOM.getDocument` → `DOM.querySelector` → `DOM.setFileInputFiles` → 回读校验
#[cfg(windows)]
fn set_file_windows(app: &AppHandle, selector: &str, file_path: &str) -> ControlResult<()> {
    // CDP 需要绝对路径(相对路径按进程 cwd 解析)
    let abs = std::path::absolute(file_path)
        .map_err(|e| format!("resolve absolute path failed: {e}"))?
        .to_string_lossy()
        .to_string();

    // 1. 文档根节点
    let doc_raw = cdp::call_json(app, "DOM.getDocument", r#"{"depth":0}"#)?;
    let doc: Value =
        serde_json::from_str(&doc_raw).map_err(|e| format!("parse DOM.getDocument: {e}"))?;
    let root_id = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(Value::as_i64)
        .ok_or("DOM.getDocument: no root nodeId")?;

    // 2. 定位 file input
    let q_params = json!({ "nodeId": root_id, "selector": selector }).to_string();
    let q_raw = cdp::call_json(app, "DOM.querySelector", &q_params)?;
    let q: Value =
        serde_json::from_str(&q_raw).map_err(|e| format!("parse DOM.querySelector: {e}"))?;
    let node_id = q.get("nodeId").and_then(Value::as_i64).unwrap_or(0);
    if node_id == 0 {
        return Err(format!("element not found: {selector}"));
    }

    // 3. 挂载文件
    let set_params = json!({ "files": [abs], "nodeId": node_id }).to_string();
    cdp::call_json(app, "DOM.setFileInputFiles", &set_params)?;

    // 4. 回读校验:files.length 与文件名必须真的挂上
    verify_files(app, selector, file_path)
}

/// 回读 `el.files` 校验上传结果;数量为 0 或文件名不含期望值时返回 Err
#[cfg(windows)]
fn verify_files(app: &AppHandle, selector: &str, file_path: &str) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          if (!el.files) return {{error: "not a file input"}};
          var names = [];
          for (var i = 0; i < el.files.length; i++) names.push(el.files[i].name);
          return {{count: el.files.length, names: names}};
        }})()"#,
        sel = selector
    );
    let v = super::eval(app, &js)?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    let count = v.get("count").and_then(Value::as_i64).unwrap_or(0);
    let names: Vec<String> = v
        .get("names")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let expected = std::path::Path::new(file_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if count < 1 || !names.iter().any(|n| n == &expected) {
        return Err(format!(
            "upload verification failed: files.length={count}, names={names:?}, expected={expected}"
        ));
    }
    Ok(())
}
