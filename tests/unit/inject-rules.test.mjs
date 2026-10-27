// inject（持久注入规则）纯逻辑单测（node:test；先 `npm run build`，测试跑 dist 产物）
import test from "node:test";
import assert from "node:assert/strict";
import {
  INJECT_FAIL_STREAK_LIMIT,
  INJECT_LIST_MAX_ITEMS,
  INJECT_MAX_RULES,
  INJECT_RULE_MAX_BYTES,
  buildInjectItems,
  formatInjectAdd,
  formatInjectBytes,
  formatInjectList,
  formatInjectTest,
  injectAddIssue,
  injectItemBytes,
  injectItemsBytes,
  injectRuleLimitIssue,
  normalizeInjectAction,
} from "../../dist/util/inject-rules.js";

test("契约常量: 规则数 ≤20 / 单规则 256KiB / failStreak 阈值 3", () => {
  assert.equal(INJECT_MAX_RULES, 20);
  assert.equal(INJECT_RULE_MAX_BYTES, 256 * 1024);
  assert.equal(INJECT_FAIL_STREAK_LIMIT, 3);
  assert.equal(INJECT_LIST_MAX_ITEMS, 5);
});

test("参数解析: 子动作规范化为小写，未给出时按只读的 list", () => {
  assert.equal(normalizeInjectAction(undefined), "list");
  assert.equal(normalizeInjectAction(""), "list");
  assert.equal(normalizeInjectAction("  ADD "), "add");
  assert.equal(normalizeInjectAction("bogus"), "bogus");
});

test("参数解析: add 缺 --match / 缺 item 分别给出原因码", () => {
  assert.equal(injectAddIssue("", 1, 0), "no_match");
  assert.equal(injectAddIssue("*a*", 0, 0), "no_items");
  assert.equal(injectAddIssue("*a*", 1, 0), null);
  assert.equal(injectAddIssue("*a*", 0, 1), null);
});

test("item 构造: --file 先于 --text，文件取 basename，文本 name 固定 text", () => {
  const items = buildInjectItems(
    [{ path: "scripts/hook.js", code: "console.log(1)" }, { path: "collect.js", code: "x" }],
    ["1+1"],
  );
  assert.deepEqual(items, [
    { kind: "text", code: "console.log(1)", name: "hook.js" },
    { kind: "text", code: "x", name: "collect.js" },
    { kind: "text", code: "1+1", name: "text" },
  ]);
});

test("item 构造: 空文件列表时只有文本项；无 basename 时回落原路径", () => {
  assert.deepEqual(buildInjectItems([], ["a"]), [{ kind: "text", code: "a", name: "text" }]);
  assert.equal(buildInjectItems([{ path: "", code: "" }], [])[0].name, "");
});

test("文件读取限制: 字节按 UTF-8 计，超 256KiB 才拒绝（边界不误杀）", () => {
  const atLimit = [{ kind: "text", code: "a".repeat(INJECT_RULE_MAX_BYTES), name: "a.js" }];
  const over = [{ kind: "text", code: "a".repeat(INJECT_RULE_MAX_BYTES + 1), name: "a.js" }];
  assert.equal(injectItemsBytes(atLimit), INJECT_RULE_MAX_BYTES);
  assert.equal(injectRuleLimitIssue(atLimit), null);
  assert.deepEqual(injectRuleLimitIssue(over), { kb: 256 });
  // 中文一个字 3 字节：字符数不等于字节数
  assert.equal(injectItemBytes({ kind: "text", code: "中", name: "t" }), 3);
  assert.equal(injectItemsBytes([{ kind: "text", code: "中", name: "a" }, { kind: "text", code: "a", name: "b" }]), 4);
});

test("输出格式化: list 表格含六列，明细只给 name + 字节数（不泄露脚本内容）", () => {
  const data = {
    rules: [
      {
        id: "r1", match: "*a.com*", enabled: true, order: 10,
        items: [{ kind: "text", code: "SECRET_CODE()", name: "hook.js" }],
        last: { ts: 1, ok: 2, failed: 0, error: "" }, failStreak: 0,
      },
      {
        id: "r2", match: "re:^https://x", enabled: false, order: 20, items: [],
        last: { ts: 1, ok: 0, failed: 1, error: "timeout\nstack line here" }, failStreak: 3,
      },
    ],
  };
  const out = formatInjectList(data);
  assert.match(out, /^inject: 2 rule\(s\), limit 20$/m);
  assert.match(out, /^id\s+order\s+enabled\s+match\s+items\s+last$/m);
  assert.match(out, /^r1\s+10\s+on\s+\*a\.com\*\s+1\s+2 ok \/ 0 failed$/m);
  assert.match(out, /^r2\s+20\s+off\s+re:\^https:\/\/x\s+0\s+0 ok \/ 1 failed\s+error: timeout\s+failStreak=3$/m);
  assert.match(out, /^ {2}r1: hook\.js 13B$/m);
  assert.ok(!out.includes("SECRET_CODE"), "list 不能输出脚本内容");
  assert.ok(!out.includes("stack line here"), "错误只取首行");
});

test("输出格式化: list 明细超过上限折叠为 +N more；无 last 时为 -", () => {
  const items = Array.from({ length: INJECT_LIST_MAX_ITEMS + 2 }, (_v, i) => ({
    kind: "text", code: "x", name: `f${i}.js`,
  }));
  const out = formatInjectList({ rules: [{ id: "r1", match: "*a*", items }] });
  assert.ok(out.includes("f0.js 1B"));
  assert.ok(out.includes("+2 more"));
  assert.match(out, /^r1\s+-\s+on\s+\*a\*\s+7\s+-$/m);
});

test("输出格式化: 空规则列表只给汇总行", () => {
  assert.equal(formatInjectList({ rules: [] }), "inject: 0 rule(s), limit 20");
  assert.equal(formatInjectList(undefined), "inject: 0 rule(s), limit 20");
});

test("输出格式化: add 返回 id + match + order + enabled + items 数", () => {
  assert.equal(
    formatInjectAdd({ id: "r3", match: "*b*", enabled: false, order: 30, items: [{ kind: "text", code: "x", name: "a.js" }] }),
    "inject: added r3  match=*b*  order=30  enabled=false  items=1",
  );
  assert.equal(formatInjectAdd(undefined), "inject: added");
});

test("输出格式化: test 只报命中规则与 item 数（不执行）", () => {
  assert.match(formatInjectTest({ matched: ["r1", "r2"], items: 3 }), /2 rule\(s\) matched \[r1, r2\], 3 item\(s\) \(not executed\)/);
  assert.match(formatInjectTest({ matched: [], items: 0 }), /no rule matched \(not executed\)/);
  assert.match(formatInjectTest(undefined), /no rule matched/);
});

test("输出格式化: last.error 里的本机路径被脱敏（不回显绝对路径）", () => {
  const out = formatInjectList({
    rules: [{ id: "r1", match: "*a*", items: [], last: { failed: 1, error: "SyntaxError at C:\\Users\\me\\hook.js:1" } }],
  });
  assert.ok(out.includes("<path>"));
  assert.ok(!out.includes("Users"), "绝对路径必须掩码");
});

test("字节展示: B / KiB 两种单位", () => {
  assert.equal(formatInjectBytes(0), "0B");
  assert.equal(formatInjectBytes(512), "512B");
  assert.equal(formatInjectBytes(1024), "1.0KiB");
  assert.equal(formatInjectBytes(1300), "1.3KiB");
});
