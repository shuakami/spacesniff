#!/usr/bin/env node
// Generates docs/images/banner.png from scripts/banner-template.html
// (icon composition on a paper-shaders grain gradient, no text).
//
// Usage: node scripts/generate-banner.mjs [output.png]
// Requires playwright-core and a Chrome reachable over CDP (CDP_URL,
// default http://localhost:29229), or playwright with a bundled chromium
// (set LAUNCH=1).
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, copyFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, dirname, resolve, extname } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = process.argv[2] ?? join(repoRoot, "docs", "images", "banner.png");

// Agent icons from @lobehub/icons static SVG CDN.
const AGENTS = [
  "claude-color",
  "openai",
  "gemini-color",
  "cursor",
  "copilot-color",
  "windsurf",
  "devin",
  "deepseek-color",
  "qwen-color",
];
const cdn = (id) => `https://unpkg.com/@lobehub/icons-static-svg@latest/icons/${id}.svg`;

const agentsHtml = AGENTS.map((id) => `<div class="agent"><img src="${cdn(id)}" /></div>`).join(
  "\n"
);

const html = readFileSync(join(repoRoot, "scripts", "banner-template.html"), "utf8")
  .replace("{{AGENTS}}", agentsHtml)
  .replace("{{SNIFFER_ICON}}", "spacesniffer-icon.png")
  .replace("{{SHADER_URL}}", "./paper-shaders-0.0.77.js");

const serveDir = mkdtempSync(join(tmpdir(), "spacesniff-banner-"));
writeFileSync(join(serveDir, "banner.html"), html);
copyFileSync(
  join(repoRoot, "docs", "images", "spacesniffer-icon.png"),
  join(serveDir, "spacesniffer-icon.png")
);
copyFileSync(
  join(repoRoot, "scripts", "vendor", "paper-shaders-0.0.77.js"),
  join(serveDir, "paper-shaders-0.0.77.js")
);

const MIME = { ".html": "text/html", ".js": "text/javascript", ".png": "image/png" };
const server = createServer((req, res) => {
  const file = join(serveDir, req.url.split("?")[0].replace(/^\//, "") || "banner.html");
  try {
    res.setHeader("Content-Type", MIME[extname(file)] ?? "application/octet-stream");
    res.end(readFileSync(file));
  } catch {
    res.statusCode = 404;
    res.end();
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const baseUrl = `http://127.0.0.1:${server.address().port}/banner.html`;

const { chromium } = await import("playwright-core");
let browser;
if (process.env.LAUNCH === "1") {
  const pw = await import("playwright");
  browser = await pw.chromium.launch({ args: ["--allow-file-access-from-files"] });
} else {
  browser = await chromium.connectOverCDP(process.env.CDP_URL ?? "http://localhost:29229");
}
const context = browser.contexts()[0] ?? (await browser.newContext());
const page = await context.newPage();
await page.setViewportSize({ width: 1700, height: 760 });
await page.goto(baseUrl);
await page.waitForFunction("window.__shaderReady === true", null, { timeout: 30000 }).catch(() => {});
await page.waitForTimeout(3000);
mkdirSync(dirname(output), { recursive: true });
await page.locator(".banner").screenshot({ path: output, scale: "device", omitBackground: true });
await page.close();
if (process.env.LAUNCH === "1") await browser.close();
server.close();
console.log(output);
