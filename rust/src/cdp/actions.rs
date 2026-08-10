//! CDP 页面操作层:在 CdpBrowser 基础上扩展交互操作
//! 交互通过 Runtime.evaluate 注入 JS 实现(与 Playwright 行为对齐)
use serde_json::{json, Value};
use std::time::Duration;

use super::browser::CdpBrowser;

impl CdpBrowser {
    /// 点击元素(注入 JS 触发真实事件)
    pub async fn click(&self, selector: &str) -> Result<(), String> {
        self.evaluate(&format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) throw new Error('Element not found'); el.scrollIntoView({{block:'center'}}); const r = el.getBoundingClientRect(); el.dispatchEvent(new MouseEvent('mousedown', {{bubbles:true, clientX:r.x+r.width/2, clientY:r.y+r.height/2}})); el.dispatchEvent(new MouseEvent('mouseup', {{bubbles:true}})); el.dispatchEvent(new MouseEvent('click', {{bubbles:true, cancelable:true}})); return true; }})()",
            sel = serde_json::to_string(selector).unwrap_or_default()
        ))
        .await?;
        Ok(())
    }

    /// 填写输入框(设置值 + 触发 input/change)
    pub async fn fill(&self, selector: &str, value: &str) -> Result<(), String> {
        self.evaluate(&format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) throw new Error('Element not found'); const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; const setter = Object.getOwnPropertyDescriptor(proto, 'value').set; setter.call(el, {val}); el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()",
            sel = serde_json::to_string(selector).unwrap_or_default(),
            val = serde_json::to_string(value).unwrap_or_default()
        ))
        .await?;
        Ok(())
    }

    /// 选择下拉选项
    pub async fn select_option(&self, selector: &str, value: &str) -> Result<(), String> {
        self.evaluate(&format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) throw new Error('Element not found'); const opt = Array.from(el.options).find(o => o.value === {val} || o.text === {val}); if (!opt) throw new Error('Option not found'); el.value = opt.value; el.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()",
            sel = serde_json::to_string(selector).unwrap_or_default(),
            val = serde_json::to_string(value).unwrap_or_default()
        ))
        .await?;
        Ok(())
    }

    /// 悬停元素
    pub async fn hover(&self, selector: &str) -> Result<(), String> {
        self.evaluate(&format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) throw new Error('Element not found'); const r = el.getBoundingClientRect(); el.dispatchEvent(new MouseEvent('mouseover', {{bubbles:true, clientX:r.x+r.width/2, clientY:r.y+r.height/2}})); el.dispatchEvent(new MouseEvent('mouseenter', {{bubbles:true}})); el.dispatchEvent(new MouseEvent('mousemove', {{bubbles:true, clientX:r.x+r.width/2, clientY:r.y+r.height/2}})); return true; }})()",
            sel = serde_json::to_string(selector).unwrap_or_default()
        ))
        .await?;
        Ok(())
    }

    /// 按键(需要先聚焦)
    pub async fn press_key(&self, selector: Option<&str>, key: &str) -> Result<(), String> {
        let focus_js = match selector {
            Some(sel) => format!(
                "const el = document.querySelector({sel}); if (!el) throw new Error('Element not found'); el.focus();",
                sel = serde_json::to_string(sel).unwrap_or_default()
            ),
            None => String::new(),
        };
        let key_js = match key {
            "Enter" => "key: 'Enter', code: 'Enter', keyCode: 13".to_string(),
            "Tab" => "key: 'Tab', code: 'Tab', keyCode: 9".to_string(),
            "Escape" => "key: 'Escape', code: 'Escape', keyCode: 27".to_string(),
            "Backspace" => "key: 'Backspace', code: 'Backspace', keyCode: 8".to_string(),
            "Delete" => "key: 'Delete', code: 'Delete', keyCode: 46".to_string(),
            "ArrowUp" => "key: 'ArrowUp', code: 'ArrowUp', keyCode: 38".to_string(),
            "ArrowDown" => "key: 'ArrowDown', code: 'ArrowDown', keyCode: 40".to_string(),
            "ArrowLeft" => "key: 'ArrowLeft', code: 'ArrowLeft', keyCode: 37".to_string(),
            "ArrowRight" => "key: 'ArrowRight', code: 'ArrowRight', keyCode: 39".to_string(),
            _ => {
                // 单字符按键
                format!("key: {k}, code: {k}, keyCode: {k}.toUpperCase().charCodeAt(0)", k = serde_json::to_string(key).unwrap_or_default())
            }
        };
        self.evaluate(&format!(
            "(() => {{ {focus_js}
            const active = document.activeElement; if (!active) throw new Error('No focused element');
            const kd = new KeyboardEvent('keydown', {{bubbles:true, cancelable:true, {key_js}}});
            const ku = new KeyboardEvent('keyup', {{bubbles:true, cancelable:true, {key_js}}});
            active.dispatchEvent(kd); active.dispatchEvent(ku);
            return true; }})()"
        ))
        .await?;
        Ok(())
    }

    /// 拖拽元素到目标
    pub async fn drag(&self, source_selector: &str, target_selector: &str) -> Result<(), String> {
        self.evaluate(&format!(
            "(() => {{ const src = document.querySelector({src}); const tgt = document.querySelector({tgt}); if (!src || !tgt) throw new Error('Element not found');
            const sr = src.getBoundingClientRect(); const tr = tgt.getBoundingClientRect();
            const sx = sr.x+sr.width/2, sy = sr.y+sr.height/2, tx = tr.x+tr.width/2, ty = tr.y+tr.height/2;
            src.dispatchEvent(new DragEvent('dragstart', {{bubbles:true, clientX:sx, clientY:sy}}));
            tgt.dispatchEvent(new DragEvent('dragover', {{bubbles:true, clientX:tx, clientY:ty}}));
            tgt.dispatchEvent(new DragEvent('drop', {{bubbles:true, clientX:tx, clientY:ty}}));
            src.dispatchEvent(new DragEvent('dragend', {{bubbles:true, clientX:tx, clientY:ty}}));
            return true; }})()",
            src = serde_json::to_string(source_selector).unwrap_or_default(),
            tgt = serde_json::to_string(target_selector).unwrap_or_default()
        ))
        .await?;
        Ok(())
    }

    /// 上传文件(通过 DOM.setFileInputFiles)
    pub async fn upload_file(&self, selector: &str, file_path: &str) -> Result<(), String> {
        // 先找 input 的 nodeId
        let expr = format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) return null; return el.nodeId = 0; }})()",
            sel = serde_json::to_string(selector).unwrap_or_default()
        );
        let _ = expr;
        // 通过 evaluate 获取对象引用不可行,用 CDP DOM 域:先 querySelector 拿到 nodeId
        let doc = self.client().send("DOM.getDocument", json!({})).await?;
        let root_node_id = doc.get("root").and_then(|r| r.get("nodeId")).and_then(|n| n.as_u64()).unwrap_or(0);
        let q = self
            .client()
            .send(
                "DOM.querySelector",
                json!({ "nodeId": root_node_id, "selector": selector }),
            )
            .await?;
        let node_id = q.get("nodeId").and_then(|n| n.as_u64()).unwrap_or(0);
        if node_id == 0 {
            return Err("Element not found".into());
        }
        self.client()
            .send(
                "DOM.setFileInputFiles",
                json!({ "nodeId": node_id, "files": [file_path] }),
            )
            .await?;
        Ok(())
    }

    /// 等待元素出现
    pub async fn wait_for_selector(&self, selector: &str, timeout_ms: u64) -> Result<bool, String> {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let expr = format!(
            "!!document.querySelector({sel})",
            sel = serde_json::to_string(selector).unwrap_or_default()
        );
        while std::time::Instant::now() < deadline {
            if let Ok(v) = self.evaluate(&expr).await {
                if v.as_bool().unwrap_or(false) {
                    return Ok(true);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("Element not found within timeout".into())
    }

    /// 滚动
    pub async fn scroll(&self, dx: f64, dy: f64) -> Result<(), String> {
        let _ = self
            .evaluate(&format!("window.scrollBy({{top:{dy}, left:{dx}, behavior:'auto'}})"))
            .await?;
        Ok(())
    }

    /// 滚动到元素
    pub async fn scroll_to_element(&self, selector: &str) -> Result<(), String> {
        let _ = self
            .evaluate(&format!(
                "(() => {{ const el = document.querySelector({sel}); if (el) el.scrollIntoView({{behavior:'smooth', block:'center'}}); return true; }})()",
                sel = serde_json::to_string(selector).unwrap_or_default()
            ))
            .await?;
        Ok(())
    }

    /// 元素状态
    pub async fn element_state(&self, selector: &str) -> Result<Value, String> {
        self.evaluate(&format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) return {{exists:false}}; const r = el.getBoundingClientRect(); const st = getComputedStyle(el); return {{exists:true, visible: r.width>0 && r.height>0 && st.visibility!=='hidden' && st.display!=='none', tag: el.tagName.toLowerCase(), text: (el.textContent||'').trim().slice(0,100), rect: {{x:Math.round(r.x), y:Math.round(r.y), w:Math.round(r.width), h:Math.round(r.height)}}, disabled: el.disabled || false}}; }})()",
            sel = serde_json::to_string(selector).unwrap_or_default()
        ))
        .await
    }

    /// 下拉选项
    pub async fn dropdown_options(&self, selector: &str) -> Result<Value, String> {
        self.evaluate(&format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el || el.tagName !== 'SELECT') return null; return Array.from(el.options).map(o => ({{value:o.value, text:o.text, selected:o.selected}})); }})()",
            sel = serde_json::to_string(selector).unwrap_or_default()
        ))
        .await
    }

    /// 可见 HTML
    pub async fn visible_html(&self, selector: Option<&str>, remove_scripts: bool, remove_comments: bool, max_len: usize) -> Result<String, String> {
        let v = self
            .evaluate(&format!(
                "(() => {{ const root = {sel} ? document.querySelector({sel}) : document.body; if (!root) return 'Element not found'; const clone = root.cloneNode(true); {rm_scripts} {rm_comments} return clone.outerHTML || clone.innerHTML || ''; }})()",
                sel = selector.map(|s| serde_json::to_string(s).unwrap_or_default()).unwrap_or_else(|| "null".into()),
                rm_scripts = if remove_scripts { "clone.querySelectorAll('script,style,link[rel=stylesheet]').forEach(e => e.remove());".to_string() } else { String::new() },
                rm_comments = if remove_comments { "const walker = document.createTreeWalker(clone, NodeFilter.SHOW_COMMENT); while (walker.nextNode()) walker.currentNode.remove();".to_string() } else { String::new() },
            ))
            .await?;
        let html = v.as_str().unwrap_or("").to_string();
        if html.len() > max_len {
            Ok(format!("{}\n\n(truncated, total length {})", &html[..max_len], html.len()))
        } else {
            Ok(html)
        }
    }

    /// 设置视口大小
    pub async fn resize(&self, width: u64, height: u64) -> Result<(), String> {
        self.client()
            .send(
                "Emulation.setDeviceMetricsOverride",
                json!({ "width": width, "height": height, "deviceScaleFactor": 0, "mobile": false }),
            )
            .await?;
        Ok(())
    }

    /// 设置 User-Agent
    pub async fn set_user_agent(&self, ua: &str) -> Result<(), String> {
        self.client()
            .send("Network.enable", json!({}))
            .await?;
        self.client()
            .send(
                "Network.setUserAgentOverride",
                json!({ "userAgent": ua }),
            )
            .await?;
        Ok(())
    }

    /// 生成 PDF(base64)
    pub async fn pdf(&self) -> Result<String, String> {
        let r = self
            .client()
            .send("Page.printToPDF", json!({ "printBackground": true }))
            .await?;
        r.get("data")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or("pdf no data".into())
    }
}
