// 页面桥:在页面 Webview 中暴露给 Rust eval 调用的工具函数
// 零注入:脚本随 Webview 创建时注入,页面 DOM 无残留脚本
// Rust 通过 webview.eval_with_callback("window.__pwPage.query(150,150)", cb) 调用

/** elementFromPoint + selector 生成(与 V3 annotate 逻辑一致) */
function queryElement(x: number, y: number): unknown {
  const el = document.elementFromPoint(x, y);
  if (!el || el === document.documentElement || el === document.body) return null;
  const r = el.getBoundingClientRect();
  return {
    selector: buildSelector(el),
    rect: { x: r.x, y: r.y, w: r.width, h: r.height },
    tag: el.tagName.toLowerCase(),
  };
}

/** 生成 CSS selector:优先 id,其次 class,最后 nth-child 路径 */
function buildSelector(node: Element): string {
  const parts: string[] = [];
  let cur: Element | null = node;
  while (cur && cur.nodeType === 1 && cur !== document.documentElement) {
    const tag = cur.tagName.toLowerCase();
    let part = tag;
    if (cur.id) {
      part += "#" + escapeCss(cur.id);
      parts.unshift(part);
      break;
    }
    const classes = Array.from(cur.classList).slice(0, 2).map(escapeCss);
    if (classes.length > 0) part += "." + classes.join(".");
    const curTag: string = cur.tagName;
    const parent: Element | null = cur.parentElement;
    if (parent) {
      const siblings: Element[] = Array.from(parent.children);
      const sameTag = siblings.filter((c) => c.tagName === curTag);
      if (sameTag.length > 1) {
        const idx = siblings.indexOf(cur) + 1;
        part += `:nth-child(${idx})`;
      }
    }
    parts.unshift(part);
    cur = parent;
  }
  return parts.join(" > ");
}

function escapeCss(s: string): string {
  return s.replace(/([^a-zA-Z0-9_-])/g, "\\$1");
}

/** 页面状态快照(截图前/批注时使用) */
function pageState(): unknown {
  return {
    url: location.href,
    title: document.title,
    readyState: document.readyState,
    viewport: { w: innerWidth, h: innerHeight, dpr: devicePixelRatio },
  };
}

// 挂载到 window:Rust eval 调用
const api = { query: queryElement, state: pageState, selector: buildSelector };
(window as unknown as { __pwPage: typeof api }).__pwPage = api;
console.log("[page-bridge] ready");
