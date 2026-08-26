// 前端构建脚本:esbuild 打包 TS → dist/ + 复制 HTML 模板
// 入口:index.ts(面板)、overlay.ts(覆盖层)、page-bridge.ts(页面桥)
import { build, context } from "esbuild";
import { rmSync, mkdirSync, copyFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outdir = resolve(root, "dist");
const srcdir = resolve(root, "src");

/** 打包入口列表 */
const entries = ["index", "overlay", "page-bridge", "toolbar", "newtab"];

/** 递归复制 HTML 模板到 dist(保持目录结构) */
function copyHtml(dir, dest) {
  mkdirSync(dest, { recursive: true });
  for (const name of readdirSync(dir)) {
    const src = join(dir, name);
    const dst = join(dest, name);
    if (statSync(src).isDirectory()) {
      copyHtml(src, dst);
    } else if (name.endsWith(".html")) {
      copyFileSync(src, dst);
      console.log("[frontend] copy", dst);
    }
  }
}

async function run() {
  rmSync(outdir, { recursive: true, force: true });
  mkdirSync(outdir, { recursive: true });

  const opts = {
    entryPoints: entries.map((e) => `src/${e}.ts`),
    bundle: true,
    format: "iife",
    target: "es2020",
    platform: "browser",
    outdir,
    entryNames: "[name]",
    minify: process.env.NODE_ENV === "production",
    logLevel: "info",
  };

  if (process.argv.includes("--watch")) {
    const ctx = await context(opts);
    await ctx.watch();
    console.log("[frontend] watching...");
  } else {
    await build(opts);
    copyHtml(srcdir, outdir);
    console.log("[frontend] build done ->", outdir);
  }
}

run().catch((e) => {
  console.error(e);
  process.exit(1);
});
