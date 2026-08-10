/**
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

export const CDN_MIRRORS: string[] = [
  "https://cdn.playwright.dev/dbazure/download/playwright",
  "https://playwright.download.prss.microsoft.com/dbazure/download/playwright",
  "https://cdn.playwright.dev"
];

export const BROWSER_MANIFEST: Record<string, BrowserDownloadEntry> = {
  "chromium": {
    "name": "chromium",
    "revision": "1228",
    "browserVersion": "149.0.7827.55",
    "download": {
      "win64": {
        "path": "builds/cft/{browserVersion}/win64/chrome-win64.zip",
        "mirrors": [
          "https://cdn.playwright.dev/dbazure/download/playwright",
          "https://playwright.download.prss.microsoft.com/dbazure/download/playwright",
          "https://cdn.playwright.dev"
        ]
      },
      "linuxX64": {
        "path": "builds/cft/{browserVersion}/linux64/chrome-linux64.zip",
        "mirrors": [
          "https://cdn.playwright.dev/dbazure/download/playwright",
          "https://playwright.download.prss.microsoft.com/dbazure/download/playwright",
          "https://cdn.playwright.dev"
        ]
      }
    }
  },
  "firefox": {
    "name": "firefox",
    "revision": "1532",
    "browserVersion": "151.0",
    "download": {
      "win64": {
        "path": "builds/firefox/{revision}/firefox-win64.zip",
        "mirrors": null
      },
      "linuxX64": {
        "path": "builds/firefox/{revision}/firefox-ubuntu-24.04.zip",
        "mirrors": null
      }
    }
  }
};
