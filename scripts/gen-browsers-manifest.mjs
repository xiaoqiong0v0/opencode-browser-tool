/**
 * 生成浏览器清单(开发时运行):从 playwright-core 提取浏览器版本 + 下载路径模板
 * 输出: src/browsers/manifest.generated.ts
 * 运行: node scripts/gen-browsers-manifest.mjs
 */
import { readFileSync, writeFileSync, existsSync } from "fs";
import { resolve, dirname, join } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const coreBundle = resolve(__dirname, "../node_modules/playwright-core/lib/coreBundle.js");

if (!existsSync(coreBundle)) {
  console.error("playwright-core not found, run: npm install");
  process.exit(1);
}

const src = readFileSync(coreBundle, "utf-8");
const browsersJson = JSON.parse(
  readFileSync(resolve(__dirname, "../node_modules/playwright-core/browsers.json"), "utf-8"),
);

// 提取 PLAYWRIGHT_CDN_MIRRORS
const mirrorsMatch = src.match(/PLAYWRIGHT_CDN_MIRRORS = (\[[\s\S]*?\]);/);
const mirrors = mirrorsMatch ? eval(mirrorsMatch[1]) : ["https://cdn.playwright.dev"];

// 提取 DOWNLOAD_PATHS(简化:只取 chromium/firefox 的 win64/linux x64)
const dlMatch = src.match(/DOWNLOAD_PATHS = (\{[\s\S]*?\n    \});/);
if (!dlMatch) {
  console.error("cannot find DOWNLOAD_PATHS in playwright-core");
  process.exit(1);
}
// 用 Function 构造器安全求值(源码来自可信依赖),注入 cftUrl 辅助函数
const downloadPaths = new Function(
  `function cftUrl(suffix) {
    return ({ browserVersion }) => ({
      path: \`builds/cft/\${browserVersion}/\${suffix}\`,
      mirrors: ${JSON.stringify(mirrors)},
    });
  }
  return ${dlMatch[1]}`,
)();

// CFT 路径是函数形式的 { path, mirrors },统一为模板字符串
function toTemplate(v) {
  if (typeof v === "function") {
    const r = v({ browserVersion: "__BV__" });
    return { path: r.path.replace("__BV__", "{browserVersion}"), mirrors: r.mirrors };
  }
  if (typeof v === "string") return { path: v.replace("%s", "{revision}"), mirrors: null };
  return null;
}

const browsers = {};
for (const b of browsersJson.browsers) {
  if (!["chromium", "firefox"].includes(b.name)) continue;
  const paths = downloadPaths[b.name] || {};
  const entry = {
    name: b.name,
    revision: b.revision,
    browserVersion: b.browserVersion,
    download: {
      win64: toTemplate(paths["win64"]),
      linuxX64: toTemplate(paths["ubuntu24.04-x64"] || paths["ubuntu22.04-x64"] || paths["debian12-x64"]),
    },
  };
  browsers[b.name] = entry;
}

const out = `/**
 * 浏览器清单(自动生成,勿手改)
 * 生成: node scripts/gen-browsers-manifest.mjs
 * 镜像规则: {mirror}/builds/cft/{browserVersion}/{platform}/{file}(CFT 格式)
 *           {mirror}/builds/{name}/{revision}/{file}(旧格式,revision 占位)
 */
export interface BrowserDownloadEntry {
  name: string;
  revision: string;
  browserVersion: string;
  download: {
    win64: { path: string; mirrors: string[] | null } | null;
    linuxX64: { path: string; mirrors: string[] | null } | null;
  };
}

export const CDN_MIRRORS: string[] = ${JSON.stringify(mirrors, null, 2)};

export const BROWSER_MANIFEST: Record<string, BrowserDownloadEntry> = ${JSON.stringify(browsers, null, 2)};
`;

writeFileSync(resolve(__dirname, "../src/browsers/manifest.generated.ts"), out, "utf-8");
console.log("manifest written: src/browsers/manifest.generated.ts");
