import { post } from "./utils";
import * as state from "./state";
import { finishAnnotation, closeEditBox, refreshRecords, sendAll } from "./core";
import type { Dom } from "./handlers";

let d: Dom;

export function initAnnotate(doms: Dom) {
  d = doms;
}

export async function pickElement(el: HTMLElement) {
  if (state.pendingEl) finishAnnotation(true);
  state.setFrozen(true);

  const tag = el.outerHTML ? el.outerHTML.split(">")[0] + ">" : `<${el.tagName.toLowerCase()}>`;
  const content = (el.textContent || "").trim().slice(0, 200);
  const selector = genSelector(el);

  const resp = await post("/annotate", { tag, content, selector, pageUrl: window.location.href });
  openEditBox(el, resp.id, "", tag);
  state.setFrozen(false);
}

function openEditBox(el: HTMLElement, id: number, annotation: string, tag: string) {
  d.editTextarea.value = annotation;
  d.editBox.classList.add("active");
  d.editTextarea.focus();

  if (!(d.editBox as any)._bound) {
    (d.editBox as any)._bound = true;
    d.editTextarea.addEventListener("input", () => {
      const hasText = d.editTextarea.value.trim().length > 0;
      d.editConfirmBtn.disabled = !hasText;
      d.editSendBtn.disabled = !hasText;
    });
    d.editConfirmBtn.addEventListener("click", () => finishAnnotation(true));
    d.editCancelBtn.addEventListener("click", () => finishAnnotation(false));
    d.editDelBtn.addEventListener("click", () => {
      if (!state.pendingEl) return;
      post("/annotate/discard", { id: state.pendingEl.id });
      state.setPendingEl(null);
      closeEditBox();
      refreshRecords();
    });
    d.editSendBtn.addEventListener("click", sendAll);
    d.editBox.addEventListener("click", (e) => {
      if (e.target === d.editBox) finishAnnotation(true);
    });
  }
  state.setPendingEl({ id });
  refreshRecords();
}

function genSelector(el: HTMLElement): string {
  if (el.id) return "#" + CSS.escape(el.id);
  for (const attr of ["data-uid", "data-testid", "data-test-id", "data-qa", "data-cy"]) {
    const val = el.getAttribute(attr);
    if (val) return `[${attr}="${CSS.escape(val)}"]`;
  }
  const name = el.getAttribute("name");
  if (name) return `${el.tagName.toLowerCase()}[name="${CSS.escape(name)}"]`;
  // nth-of-type 路径
  const parts: string[] = [];
  let cur: HTMLElement | null = el;
  while (cur && cur !== document.body && cur !== document.documentElement) {
    const tag = cur.tagName.toLowerCase();
    const parent = cur.parentElement;
    if (parent) {
      const sameTag = Array.from(parent.children).filter((c) => c.tagName === cur!.tagName);
      const nth = sameTag.indexOf(cur) + 1;
      parts.unshift(`${tag}:nth-of-type(${nth})`);
    } else {
      parts.unshift(tag);
      break;
    }
    cur = parent;
  }
  return parts.join(" > ");
}
