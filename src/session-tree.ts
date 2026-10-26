// 会话树归并(纯逻辑,便于单测):把子代理会话归并到根会话的浏览器实例。
//
// 背景:浏览器实例按 session 隔离,但 task 子代理会话也是 session。若直接用钩子里的
// sessionID 作路由键,子代理一调 bt_cli 就会另起一个 shell 与数据目录。
// 目标语义(用户确认):
//   1. 默认归并到根会话:沿 Session.parentID 回溯到最顶层,以根 sid 作 browserKey;
//   2. --private-session:用调用者自己的 sid 建私有实例,并且**粘住**(之后不带标志也继续私有);
//   3. 根会话自己带 --private-session 等价于自己,不报错。
//
// 失败/超时不抛异常:回退 raw sid,且**不缓存**该结果(下一次调用可重试)。

/** 单跳父会话查询:返回父 sid;`null` = 确认无父(根);`undefined` = 查询失败(超时/异常/无数据) */
export type ParentFetcher = (sid: string) => Promise<string | null | undefined>;

/** 会话树归并配置 */
export interface SessionTreeOptions {
  /** 查询某会话的 parentID(插件侧包 client.session.get) */
  fetchParent: ParentFetcher;
  /** 单跳查询超时(ms);超时按失败处理,默认 1500 */
  timeoutMs?: number;
  /** 最大回溯跳数(防御异常数据/超长链),默认 5 */
  maxHops?: number;
  /** 告警日志(失败/超时/成环/超限);默认静默 */
  onWarn?: (message: string) => void;
}

/** 会话树归并器 */
export interface SessionTree {
  /**
   * 取该 raw 会话最终使用的浏览器 key:
   * - 私有会话(粘住)= 自身;
   * - 否则沿 parentID 回溯到根会话;
   * - 回溯失败/超时/成环/超限 = 回退 raw(失败结果不缓存)。
   */
  browserKey(raw: string): Promise<string>;
  /** 同步取「已知」的浏览器 key(未解析/失败时回退 raw);供同步兜底解析器使用 */
  knownKey(raw: string): string;
  /** 标记 raw 会话为私有(粘住):此后 browserKey(raw) 恒为 raw */
  markPrivate(raw: string): void;
  /** raw 是否已被标记为私有 */
  isPrivate(raw: string): boolean;
  /** 会话删除后清理:自身缓存/私有标记 + 所有映射到它的后代条目 */
  forget(sid: string): void;
  /** 已解析的根(诊断/测试用;未解析返回 undefined) */
  cachedRoot(raw: string): string | undefined;
}

/** 默认单跳查询超时(ms) */
const DEFAULT_QUERY_TIMEOUT_MS = 1500;
/** 默认最大回溯跳数 */
const DEFAULT_MAX_HOPS = 5;
/** 拿不到任何 sid 时的兜底桶(与 client.resolveSessionId 同口径) */
const DEFAULT_SESSION_BUCKET = "default";

/** 规范化会话 id:去空白;空值落到 default 桶 */
function normalizeSid(raw: string): string {
  const sid = (raw || "").trim();
  return sid || DEFAULT_SESSION_BUCKET;
}

/** 创建会话树归并器(无副作用,不做 IO;IO 由 fetchParent 注入) */
export function createSessionTree(opts: SessionTreeOptions): SessionTree {
  const timeoutMs = opts.timeoutMs ?? DEFAULT_QUERY_TIMEOUT_MS;
  const maxHops = opts.maxHops ?? DEFAULT_MAX_HOPS;
  /** raw sid → 根 sid(仅缓存成功解析的结果) */
  const rootCache = new Map<string, string>();
  /** 已标记私有的 raw sid */
  const privateSids = new Set<string>();

  /** 有界查询 parentID;超时/异常 → undefined(区别于 null=确认无父) */
  async function fetchParentSafe(sid: string): Promise<string | null | undefined> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const query = Promise.resolve().then(() => opts.fetchParent(sid));
      const timeout = new Promise<undefined>((resolveTimeout) => {
        timer = setTimeout(() => resolveTimeout(undefined), timeoutMs);
      });
      const parent = await Promise.race([query, timeout]);
      if (parent === undefined) return undefined;
      if (parent === null) return null;
      const p = String(parent).trim();
      return p || null;
    } catch (e: any) {
      opts.onWarn?.(`[session-tree] parent lookup failed sid=${sid}: ${e?.message || String(e)}`);
      return undefined;
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  function knownKey(raw: string): string {
    const sid = normalizeSid(raw);
    if (privateSids.has(sid)) return sid;
    return rootCache.get(sid) || sid;
  }

  async function browserKey(raw: string): Promise<string> {
    const sid = normalizeSid(raw);
    if (privateSids.has(sid)) return sid;
    const cached = rootCache.get(sid);
    if (cached) return cached;

    // 沿 parentID 向上回溯;路径上的节点在成功时一并缓存
    const path: string[] = [sid];
    const seen = new Set<string>([sid]);
    let current = sid;
    for (let hop = 0; hop < maxHops; hop++) {
      const parent = await fetchParentSafe(current);
      if (parent === undefined) {
        opts.onWarn?.(`[session-tree] resolve failed at sid=${current}; fallback to raw sid=${sid}`);
        return sid; // 失败不缓存,下次重试
      }
      if (parent === null) {
        // current 是根:路径上所有节点都归并到它
        for (const node of path) rootCache.set(node, current);
        return current;
      }
      if (seen.has(parent)) {
        opts.onWarn?.(`[session-tree] cycle detected (sid=${sid}, repeated=${parent}); fallback to raw sid`);
        return sid;
      }
      if (hop === maxHops - 1) {
        opts.onWarn?.(`[session-tree] exceeded maxHops=${maxHops} (sid=${sid}); fallback to raw sid`);
        return sid;
      }
      seen.add(parent);
      path.push(parent);
      current = parent;
    }
    return sid;
  }

  function markPrivate(raw: string): void {
    const sid = (raw || "").trim();
    if (sid) privateSids.add(sid);
  }

  function isPrivate(raw: string): boolean {
    return privateSids.has((raw || "").trim());
  }

  function forget(sid: string): void {
    if (!sid) return;
    privateSids.delete(sid);
    rootCache.delete(sid);
    // 删除所有映射到该会话的后代缓存(sid 是根/某层祖先时)
    for (const [node, root] of [...rootCache]) {
      if (root === sid) rootCache.delete(node);
    }
  }

  function cachedRoot(raw: string): string | undefined {
    return rootCache.get((raw || "").trim());
  }

  return { browserKey, knownKey, markPrivate, isPrivate, forget, cachedRoot };
}
