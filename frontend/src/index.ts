// 面板 Webview 入口:批注列表 + 操作按钮 + 说明输入
// 与 Rust 通过 Tauri event/invoke 通信
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AnnotationRecord } from "./types";

/** DOM 引用集合 */
interface PanelDoms {
  btnAnnotate: HTMLButtonElement;
  btnSend: HTMLButtonElement;
  records: HTMLDivElement;
  note: HTMLTextAreaElement;
}

class Panel {
  private doms: PanelDoms;
  private records: AnnotationRecord[] = [];

  constructor() {
    this.doms = {
      btnAnnotate: document.getElementById("btn-annotate") as HTMLButtonElement,
      btnSend: document.getElementById("btn-send") as HTMLButtonElement,
      records: document.getElementById("records") as HTMLDivElement,
      note: document.getElementById("note") as HTMLTextAreaElement,
    };
    this.bindEvents();
  }

  private bindEvents(): void {
    this.doms.btnAnnotate.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "toggle-annotate" }).then((on) => {
        this.doms.btnAnnotate.classList.toggle("active", on);
      });
    });
    this.doms.btnSend.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "send-all" }).then((sent) => {
        const el = document.getElementById("send-result");
        if (el) {
          el.textContent = sent ? "已发送批注" : "无记录可发送";
          el.className = sent ? "send-ok" : "send-empty";
        }
      });
    });
    // Rust 推送记录变更
    void listen<AnnotationRecord[]>("records-changed", (e) => {
      this.records = e.payload || [];
      this.render();
    });
  }

  /** 渲染记录列表 */
  private render(): void {
    const el = this.doms.records;
    el.innerHTML = "";
    if (this.records.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = "（无记录）";
      el.appendChild(empty);
      return;
    }
    for (const r of this.records) {
      const item = document.createElement("div");
      item.className = "rec";
      item.textContent = `#${r.index} ${r.selector}`;
      if (r.note) {
        const note = document.createElement("div");
        note.className = "rec-note";
        note.textContent = r.note;
        item.appendChild(note);
      }
      el.appendChild(item);
    }
  }

  /** 主动拉取记录(启动时) */
  async refresh(): Promise<void> {
    try {
      this.records = await invoke<AnnotationRecord[]>("panel_records");
      this.render();
    } catch (e) {
      console.error("[panel] refresh failed:", e);
    }
  }
}

const panel = new Panel();
void panel.refresh();
console.log("[panel] ready");
