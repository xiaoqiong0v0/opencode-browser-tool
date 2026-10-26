// 会话树归并单测(node:test;先 `npm run build`,测试直接跑 dist 产物,不另建构建链)
import test from "node:test";
import assert from "node:assert/strict";
import { createSessionTree } from "../../dist/session-tree.js";
import { createSessionIDCache } from "../../dist/session-id.js";

/** 用固定父子表构造 ParentFetcher;calls 记录每次查询的 sid(缺失键 → 根) */
function fetcherOf(parents) {
  const calls = [];
  const fn = async (sid) => {
    calls.push(sid);
    return Object.prototype.hasOwnProperty.call(parents, sid) ? parents[sid] : null;
  };
  fn.calls = calls;
  return fn;
}

test("多级 parent 回溯到根,并缓存路径上所有节点", async () => {
  const fetchParent = fetcherOf({ child: "mid", mid: "root" });
  const tree = createSessionTree({ fetchParent });
  assert.equal(await tree.browserKey("child"), "root");
  assert.equal(tree.cachedRoot("child"), "root");
  assert.equal(tree.cachedRoot("mid"), "root");
  assert.equal(tree.cachedRoot("root"), "root");
  const calls = fetchParent.calls.length;
  // 第二次命中缓存,不再查询
  assert.equal(await tree.browserKey("child"), "root");
  assert.equal(fetchParent.calls.length, calls);
});

test("parentID 缺失(无父)时归并为自身并缓存", async () => {
  const fetchParent = fetcherOf({});
  const tree = createSessionTree({ fetchParent });
  assert.equal(await tree.browserKey("solo"), "solo");
  assert.equal(tree.cachedRoot("solo"), "solo");
});

test("成环时回退 raw 且不缓存(并告警)", async () => {
  const warns = [];
  const fetchParent = fetcherOf({ a: "b", b: "a" });
  const tree = createSessionTree({ fetchParent, onWarn: (m) => warns.push(m) });
  assert.equal(await tree.browserKey("a"), "a");
  assert.equal(tree.cachedRoot("a"), undefined);
  assert.ok(warns.some((m) => m.includes("cycle")));
});

test("超过 maxHops 回退 raw 且不缓存", async () => {
  const parents = { n0: "n1", n1: "n2", n2: "n3", n3: "n4", n4: "n5", n5: "n6", n6: "n7" };
  const warns = [];
  const tree = createSessionTree({ fetchParent: fetcherOf(parents), maxHops: 5, onWarn: (m) => warns.push(m) });
  assert.equal(await tree.browserKey("n0"), "n0");
  assert.equal(tree.cachedRoot("n0"), undefined);
  assert.ok(warns.some((m) => m.includes("maxHops")));
});

test("查询超时回退 raw,不缓存,下一次可重试成功", async () => {
  let mode = "hang";
  const fetchParent = async (sid) => {
    if (mode === "hang") return new Promise(() => {}); // 永不 resolve,由 timeout 兜底
    return sid === "child" ? "root" : null;
  };
  const warns = [];
  const tree = createSessionTree({ fetchParent, timeoutMs: 20, onWarn: (m) => warns.push(m) });
  const started = Date.now();
  assert.equal(await tree.browserKey("child"), "child");
  assert.ok(Date.now() - started < 1000, "超时应有界返回");
  assert.equal(tree.cachedRoot("child"), undefined);
  assert.ok(warns.some((m) => m.includes("resolve failed")));
  // 超时结果不缓存 → 网络恢复后可正确归并
  mode = "ok";
  assert.equal(await tree.browserKey("child"), "root");
  assert.equal(tree.cachedRoot("child"), "root");
});

test("查询抛错回退 raw 且不缓存", async () => {
  const warns = [];
  const tree = createSessionTree({
    fetchParent: async () => {
      throw new Error("boom");
    },
    onWarn: (m) => warns.push(m),
  });
  assert.equal(await tree.browserKey("child"), "child");
  assert.equal(tree.cachedRoot("child"), undefined);
  assert.ok(warns.some((m) => m.includes("parent lookup failed")));
});

test("私有会话粘住自身:标记后不再回溯", async () => {
  const fetchParent = fetcherOf({ child: "root" });
  const tree = createSessionTree({ fetchParent });
  tree.markPrivate("child");
  assert.equal(tree.isPrivate("child"), true);
  assert.equal(await tree.browserKey("child"), "child");
  assert.equal(tree.knownKey("child"), "child");
  assert.equal(tree.cachedRoot("child"), undefined);
  assert.equal(fetchParent.calls.length, 0, "私有会话不应发起父查询");
});

test("归并/私有都不改 raw 会话缓存(旁路)", async () => {
  const cache = createSessionIDCache();
  cache.updateFromHookInput({ sessionID: "child" });
  const tree = createSessionTree({ fetchParent: fetcherOf({ child: "root" }) });
  assert.equal(await tree.browserKey("child"), "root");
  assert.equal(cache.get(), "child", "归并不得改写钩子缓存");
  tree.markPrivate("child");
  assert.equal(await tree.browserKey("child"), "child");
  assert.equal(cache.get(), "child", "标记私有不得改写钩子缓存");
  cache.clearIf("child");
  assert.equal(cache.get(), "");
});

test("knownKey 同步兜底:未解析返回 raw,解析后返回根", async () => {
  const tree = createSessionTree({ fetchParent: fetcherOf({ child: "root" }) });
  assert.equal(tree.knownKey("child"), "child");
  assert.equal(await tree.browserKey("child"), "root");
  assert.equal(tree.knownKey("child"), "root");
});

test("forget 清理自身私有标记/缓存,以及映射到该 sid 的后代", async () => {
  const tree = createSessionTree({ fetchParent: fetcherOf({ child: "root", child2: "root" }) });
  await tree.browserKey("child");
  await tree.browserKey("child2");
  tree.markPrivate("child");
  tree.forget("child");
  assert.equal(tree.isPrivate("child"), false);
  assert.equal(tree.cachedRoot("child"), undefined);
  // 删根时后代映射一并清除
  tree.forget("root");
  assert.equal(tree.cachedRoot("child2"), undefined);
});

test("空 sid 落到 default 桶", async () => {
  const tree = createSessionTree({ fetchParent: fetcherOf({}) });
  assert.equal(await tree.browserKey(""), "default");
  assert.equal(tree.knownKey("  "), "default");
});
