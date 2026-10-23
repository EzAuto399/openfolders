// Reordering the row with HTML5 drag & drop, checked headless with no app running:
// serves src/ui.html locally, answers every /api/* with a mock, then drags tile 0 onto
// tile 2 and asserts the folder order changed and was saved.
// Usage: node tests/ui/dnd.mjs
import { chromium, webkit } from "playwright";
import http from "node:http";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const html = readFileSync(path.join(root, "src/ui.html"), "utf8");
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64"
);

const initialApps = [
  { name: "Alpha", path: "/Applications/Alpha.app" },
  { name: "Bravo", path: "/Applications/Bravo.app" },
  { name: "Charlie", path: "/Applications/Charlie.app" },
];
const saves = [];
const drags = [];

const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  const json = (o) => {
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify(o));
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", async () => {
    if (url.pathname === "/") {
      res.setHeader("Content-Type", "text/html");
      res.end(html);
    } else if (url.pathname === "/api/icon") {
      res.setHeader("Content-Type", "image/png");
      res.end(PNG);
    } else if (url.pathname === "/api/state") {
      json({
        config: {
          folders: [{ id: "f1", name: "Work", accent: "#6366f1", apps: initialApps }],
          active_folder_id: "f1",
        },
        running: {},
      });
    } else if (url.pathname === "/api/save-config") {
      saves.push(JSON.parse(body));
      json({ ok: true });
    } else if (url.pathname === "/api/drag") {
      drags.push(JSON.parse(body));
      json({ ok: true, removed: false, pinned: false });
    } else if (url.pathname === "/api/set-layout") {
      // Mirror the native window: it resizes to fit what's on show (main.rs place_window),
      // which it does mid-drag too, when the tooltip hides at dragstart
      const b = JSON.parse(body);
      await page.setViewportSize({ width: Math.max(80, Math.round(b.width)), height: Math.max(60, Math.round(b.height)) });
      json({ ok: true });
    } else {
      json({ ok: true });
    }
  });
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

// The app's webview is WKWebView, so WebKit is the engine that matters; Chromium is the
// control. Both must pass.
const engine = process.argv[2] || "webkit";
const browser = await (engine === "chromium" ? chromium : webkit).launch();
const page = await browser.newPage({ viewport: { width: 520, height: 128 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
// Log what the page actually sees during the gesture
await page.addInitScript(() => {
  window.__events = [];
  for (const t of ["dragstart", "dragenter", "dragover", "drop", "dragend", "dragleave"]) {
    document.addEventListener(t, (e) => window.__events.push(`${t}:${e.target.className || e.target.id}`), true);
  }
});

await page.goto(`http://127.0.0.1:${port}/`);
await page.waitForSelector(".dock-icon-item[data-idx='2']");

const before = await page.evaluate(() => [...document.querySelectorAll(".dock-icon-item")].map((el) => el.dataset.name));
console.log("row before:", before.join(", "));

await page.dragAndDrop(".dock-icon-item[data-idx='0']", ".dock-icon-item[data-idx='2']");
await page.waitForTimeout(300);

const events = await page.evaluate(() => window.__events);
const order = await page.evaluate(() => state.config.folders[0].apps.map((a) => a.name));
const saved = saves.length ? saves.at(-1).folders[0].apps.map((a) => a.name) : null;
const dragCalls = drags.filter((d) => d.active !== undefined);

const expected = ["Bravo", "Charlie", "Alpha"];
const pass =
  JSON.stringify(order) === JSON.stringify(expected) &&
  JSON.stringify(saved) === JSON.stringify(expected) &&
  errors.length === 0;

console.log("dnd events:", events.join(" | ") || "(none)");
console.log("/api/drag calls:", JSON.stringify(dragCalls));
console.log("order after:", order.join(", "), "| saved:", saved ? saved.join(", ") : "(nothing saved)");
if (errors.length) console.log("page errors:", errors.join("; "));

await browser.close();
server.close();
console.log(pass ? "PASS" : "FAIL");
process.exit(pass ? 0 : 1);
