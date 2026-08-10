export let isActive = false;
export let mode: "" | "annotate" = "";
export let pendingEl: { id: number } | null = null;
export let frozen = false;

export function setIsActive(v: boolean) {
  isActive = v;
}
export function setMode(v: "" | "annotate") {
  mode = v;
}
export function setPendingEl(v: { id: number } | null) {
  pendingEl = v;
}
export function setFrozen(v: boolean) {
  frozen = v;
}
