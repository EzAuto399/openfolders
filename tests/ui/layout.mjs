// Opens every surface of the OpenFolders UI in a headless browser that resizes itself the way
// the native window does, and checks nothing is clipped or spills out of its panel.
// Usage: node layout.mjs <port of the running app's UI server>
import { chromium } from "playwright";

const port = process.argv[2];
const browser = await chromium.launch();
let failed = 0;

for (const scheme of ["dark", "light"]) {
  const page = await browser.newPage({ viewport: { width: 520, height: 128 }, colorScheme: scheme });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  // Never change anything for real: every write is answered locally. set-layout resizes the
  // page like the native window would.
  await page.route("**/api/**", async route => {
    const req = route.request();
    if (req.method() === "POST" && req.url().endsWith("/api/set-layout")) {
      const b = JSON.parse(req.postData());
      await page.setViewportSize({ width: Math.max(80, Math.round(b.width)), height: Math.max(60, Math.round(b.height)) });
    }
    if (req.method() === "POST") return route.fulfill({ body: '{"ok":true}' });
    return route.continue();
  });
  await page.goto(`http://127.0.0.1:${port}/`);
  await page.waitForTimeout(300);
  const folderId = await page.evaluate(() => state.config.folders[0]?.id);

  const states = [
    ["row", async () => page.evaluate(id => window.__openFolderFromDock(id, 33, true), folderId)],
    ["row with hover setup button", async () => page.evaluate(id => window.__openFolderFromDock(id, 33, false), folderId)],
    ["hover setup tooltip", async () => page.hover("#hoverSetupBtn")],
    ["app name tooltip", async () => page.hover(".dock-icon-item")],
    ["large Dock (64 pt)", async () => page.evaluate(id => window.__openFolderFromDock(id, 66, true), folderId)],
    ["app menu", async () => { await page.evaluate(id => window.__openFolderFromDock(id, 33, true), folderId); await page.click(".dock-icon-item", { button: "right" }); }],
    ["folder menu", async () => { await page.keyboard.press("Escape"); await page.click("#folderMenuBtn"); }],
    ["app picker", async () => page.evaluate(() => { closeAllModals(); openAppPicker(getHoveredOrActiveFolder()); })],
    ["app picker, no results", async () => page.fill("#pickerSearch", "zzzzzz")],
    ["edit folder", async () => page.evaluate(() => { closeAllModals(); openFolderModal(getHoveredOrActiveFolder()); })],
    ["edit folder, confirm delete", async () => page.click("#deleteFolderBtn")],
    ["new folder", async () => page.evaluate(() => { closeAllModals(); openFolderModal(null); })],
    ["Dock settings", async () => page.evaluate(() => { closeAllModals(); openDockSyncModal(); })],
  ];

  for (const [name, act] of states) {
    let problems;
    try {
      await act();
      await page.waitForTimeout(350);
      problems = await page.evaluate(() => {
        const out = [];
        const vw = window.innerWidth, vh = window.innerHeight;
        const label = el => el.id ? `#${el.id}` : `.${[...el.classList].join(".") || el.tagName.toLowerCase()}`;
        const visible = el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== "hidden"; };
        const surfaces = [document.getElementById("secondRowBar"), document.getElementById("folderLabel"),
          ...document.querySelectorAll(".modal-card.open, .context-menu.open, .dock-tooltip.show")].filter(visible);
        for (const el of surfaces) {
          const r = el.getBoundingClientRect();
          if (r.left < -0.5 || r.top < -0.5 || r.right > vw + 0.5 || r.bottom > vh + 0.5)
            out.push(`${label(el)} is cut off by the window (${Math.round(r.left)},${Math.round(r.top)} ${Math.round(r.width)}×${Math.round(r.height)} in ${vw}×${vh})`);
        }
        // Nothing may spill out of a panel, except content inside a scrolling area
        for (const panel of document.querySelectorAll(".modal-card.open, .context-menu.open")) {
          const pr = panel.getBoundingClientRect();
          for (const el of panel.querySelectorAll("*")) {
            if (!visible(el)) continue;
            let scroller = el.parentElement, inScroller = false;
            while (scroller && scroller !== panel) {
              const o = getComputedStyle(scroller).overflowY;
              if (o === "auto" || o === "scroll") { inScroller = true; break; }
              scroller = scroller.parentElement;
            }
            const r = el.getBoundingClientRect();
            const outX = r.left < pr.left - 0.5 || r.right > pr.right + 0.5;
            const outY = !inScroller && (r.top < pr.top - 0.5 || r.bottom > pr.bottom + 0.5);
            if (outX || outY) out.push(`${label(el)} spills out of ${label(panel)}`);
          }
        }
        return out;
      });
    } catch (e) {
      problems = [`couldn't open it: ${e.message.split("\n")[0]}`];
    }
    problems.push(...errors.splice(0));
    const title = `${name} (${scheme})`;
    if (problems.length) {
      failed++;
      console.log(`  ✗ ${title}: ${[...new Set(problems)].slice(0, 3).join("; ")}`);
    } else {
      console.log(`  ✓ ${title}`);
    }
  }
  await page.close();
}
await browser.close();
console.log(failed ? `\n${failed} panel check${failed === 1 ? "" : "s"} failed.` : "\nAll panel checks passed.");
process.exit(failed ? 1 : 0);
