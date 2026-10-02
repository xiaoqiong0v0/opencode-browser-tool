// 会话 id 提取与缓存(纯逻辑,便于单测):投递批注时必须拿到"当前会话 id"。
// 可靠来源是插件钩子上下文(等价于参考项目 opencode-playwright-tool 的 ctx.sessionID),
// 而不是 client.session.list() —— 后者可能返回空/异常包络(见 docs/implementation.md)。
//   - event: Event.properties.sessionID;
//     session.created/updated/deleted 则挂在 properties.info.id
//   - chat.message / tool.execute.before / shell.env / 工具 execute 的 context: 入参 sessionID

/** 从 opencode 插件 event 提取 sessionID(兼容 properties / data 两种包络) */
export function extractSessionIDFromEvent(ev: any): string {
  if (!ev || typeof ev !== "object") return "";
  const p = ev.properties ?? ev.data ?? {};
  const sid = p.sessionID ?? p.info?.id;
  return typeof sid === "string" ? sid : "";
}

/** 从 chat.message / tool.execute.before / 工具 context 等入参提取 sessionID */
export function extractSessionIDFromHookInput(input: any): string {
  const sid = input?.sessionID;
  return typeof sid === "string" ? sid : "";
}

/** 当前会话 id 缓存:优先于 client.session.list(),避免 list() 为空导致投递静默失败 */
export function createSessionIDCache() {
  let current = "";
  return {
    /** 从钩子入参刷新;返回本次提取到的 id(可能为 "" 表示未提供) */
    updateFromHookInput(input: any): string {
      const sid = extractSessionIDFromHookInput(input);
      if (sid) current = sid;
      return sid;
    },
    /** 从 event 刷新(不用于 session.deleted,删除时用 clearIf) */
    updateFromEvent(ev: any): string {
      const sid = extractSessionIDFromEvent(ev);
      if (sid) current = sid;
      return sid;
    },
    get(): string {
      return current;
    },
    /** 会话被删除时清除缓存(仅当匹配) */
    clearIf(id: string): void {
      if (id && current === id) current = "";
    },
  };
}
