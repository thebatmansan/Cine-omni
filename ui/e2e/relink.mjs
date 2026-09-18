// End-to-end: media that moved, found again by its content, from the real
// application.
//
// `cine new`, `cine bars`, `cine start`, then headless Chrome: the media is
// moved on disk, the application notices it is offline, a person types the
// folder to look in, and the runtime finds the file by its content and
// relinks it — while a file of the same name and other content is refused.
//
//   CINE_BIN=target/debug/cine node ui/e2e/relink.mjs [port]
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, renameSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = Number(process.argv[2] ?? 7399);
const CDP_PORT = PORT + 1000;
const CINE = process.env.CINE_BIN ?? "target/debug/cine";
const SHOTS = process.env.CINE_E2E_SHOTS ?? tmpdir();
const CHROME = process.env.CHROME_BIN ?? "google-chrome";
if (!existsSync(CINE)) {
  console.log(`SKIP: no cine binary at ${CINE}; build it or set CINE_BIN`);
  process.exit(2);
}

const children = [];
function cleanup() {
  for (const c of children) {
    try {
      c.kill("SIGTERM");
    } catch {}
  }
}
process.on("exit", cleanup);
const assert = (cond, msg) => {
  if (!cond) {
    console.error("FAIL:", msg);
    cleanup();
    process.exit(1);
  }
  console.log("  ok ", msg);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const work = mkdtempSync(join(tmpdir(), "cine-e2e-relink-"));
const project = join(work, "film");
const cards = join(work, "cards");
mkdirSync(join(cards, "day1"), { recursive: true });
const media = join(cards, "day1", "A001C001.mov");
execFileSync(CINE, ["new", project, "--name", "Proxy E2E"], { stdio: "inherit" });
execFileSync(CINE, ["bars", media, "--seconds", "1", "--size", "640x360"], { stdio: "inherit" });

const server = spawn(CINE, ["start", project, "--port", String(PORT), "--no-window"], {
  stdio: ["ignore", "inherit", "inherit"],
});
children.push(server);
const base = `http://127.0.0.1:${PORT}`;
for (let i = 0; ; i++) {
  try {
    if ((await fetch(`${base}/health`)).ok) break;
  } catch {}
  if (i > 300) throw new Error("cine start did not come up");
  await sleep(100);
}

function rpc() {
  const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws`);
  let id = 1;
  const pending = new Map();
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    m.error ? p.reject(new Error(m.error.message)) : p.resolve(m.result);
  };
  const ready = new Promise((r, j) => {
    ws.onopen = r;
    ws.onerror = j;
  });
  return {
    ready,
    call(method, params = {}) {
      const n = id++;
      ws.send(JSON.stringify({ jsonrpc: "2.0", id: n, method, params }));
      return new Promise((resolve, reject) => pending.set(n, { resolve, reject }));
    },
    close: () => ws.close(),
  };
}
const api = rpc();
await api.ready;
const f24 = (n) => ({ ticks: n, rate_num: 24, rate_den: 1 });
const seq = await api.call("new_timeline", { name: "Assembly", rate_num: 24, rate_den: 1 });
const t = await api.call("timeline", { id: seq.id });
const asset = (await api.call("ingest", { path: media })).id;
await api.call("place", {
  tracks: [t.tracks[0].id, t.tracks[1].id],
  time: f24(0),
  clip: { media: asset, source_range: { start_time: f24(0), duration: f24(24) } },
});
await api.call("focus", { timeline: seq.id });
assert(true, "a one-second cut of the card's media is open");

// -- the browser -----------------------------------------------------------
const profile = join(work, "chrome");
const chrome = spawn(
  CHROME,
  ["--headless=new", `--remote-debugging-port=${CDP_PORT}`, `--user-data-dir=${profile}`, "--no-first-run", "--window-size=1600,1000", "about:blank"],
  { stdio: "ignore" },
);
children.push(chrome);
let target;
for (let i = 0; ; i++) {
  try {
    const list = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`)).json();
    target = list.find((x) => x.type === "page");
    if (target) break;
  } catch {}
  if (i > 200) throw new Error("Chrome did not start");
  await sleep(100);
}
const cdp = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r, j) => {
  cdp.onopen = r;
  cdp.onerror = j;
});
let cdpId = 1;
const cdpPending = new Map();
const pageErrors = [];
cdp.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === "Runtime.exceptionThrown") pageErrors.push(m.params.exceptionDetails.text);
  const p = cdpPending.get(m.id);
  if (!p) return;
  cdpPending.delete(m.id);
  m.error ? p.reject(new Error(m.error.message)) : p.resolve(m.result);
};
const send = (method, params = {}) => {
  const n = cdpId++;
  cdp.send(JSON.stringify({ id: n, method, params }));
  return new Promise((resolve, reject) => cdpPending.set(n, { resolve, reject }));
};
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(`${expression}: ${r.exceptionDetails.text}`);
  return r.result.value;
};
const until = async (expression, what, ms = 300000) => {
  const t0 = Date.now();
  for (;;) {
    const v = await evaluate(expression).catch(() => null);
    if (v) return v;
    if (Date.now() - t0 > ms) {
      await shot(`timeout-${what.replace(/\W+/g, "-")}`);
      assert(false, `${what} (timed out after ${ms} ms)`);
    }
    await sleep(200);
  }
};
const shot = async (name) => {
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  const path = join(SHOTS, `cine-e2e-relink-${name}.png`);
  writeFileSync(path, Buffer.from(data, "base64"));
  console.log(`  shot ${path}`);
};
const choose = (label, value) =>
  evaluate(`(() => {
    const el = document.querySelector('select[aria-label="${label}"]');
    if (!el) throw new Error('no select ${label}');
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(el, ${JSON.stringify(value)});
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return el.value;
  })()`);
const type = (label, value) =>
  evaluate(`(() => {
    const el = document.querySelector('input[aria-label="${label}"]');
    if (!el) throw new Error('no input ${label}');
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, ${JSON.stringify(value)});
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return el.value;
  })()`);
const clickText = (text) =>
  evaluate(`(() => {
    const b = [...document.querySelectorAll('button')].find((b) => b.textContent.trim() === ${JSON.stringify(text)});
    if (!b) throw new Error('no button ${text}');
    b.click();
    return true;
  })()`);

await send("Page.enable");
await send("Runtime.enable");
await send("Page.navigate", { url: `${base}/` });
await until(`[...document.querySelectorAll('button')].some((b) => b.textContent.startsWith('Jobs'))`, "the editor loads");
await until(
  `(() => {
     const row = [...document.querySelectorAll('.tree-row')].find((r) => r.querySelector('.label')?.textContent === 'A001C001.mov');
     if (!row) return false;
     row.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
     return true;
   })()`,
  "the media is selected in the project browser",
);
await until(`document.body.textContent.includes('Resolution')`, "the inspector shows whether it is resolved");
assert(
  await evaluate(`document.body.textContent.includes('Online at its recorded path')`),
  "it starts online at its recorded path",
);

// The card is archived somewhere else, under another name.
const moved = join(cards, "archive", "TAKE-ONE.mov");
mkdirSync(join(cards, "archive"), { recursive: true });
renameSync(media, moved);
// A different take is left where it was, with the original's name.
execFileSync(CINE, ["bars", media, "--seconds", "2", "--size", "640x360"], { stdio: "ignore" });

await clickText("Check media");
let offline = null;
for (let i = 0; i < 300 && !offline; i++) {
  offline = await evaluate(`(() => { const b = document.querySelector('.panel .bad'); return b && b.textContent; })()`).catch(() => null);
  if (!offline) await sleep(200);
}
if (!offline) {
  console.log("PANEL TEXT:", await evaluate(`document.body.textContent.slice(0, 1200)`));
  console.log("MEDIA STATE:", JSON.stringify(await api.call("media")).slice(0, 600));
}
assert(!!offline, `the application notices the media is not where it was: ${String(offline).slice(0, 140)}`);
await shot("offline");

// Look for it by content, in the card.
await type("Relink folder", cards);
const began = Date.now();
await clickText("Find by content");
const back = await until(
  `(() => {
     const t = document.body.textContent;
     return t.includes('Online at its recorded path') && t.includes('relinked from') && t;
   })()`,
  "the media is found by its content and relinked",
);
console.log(`MEASURED type a folder to relinked, in the browser: ${Date.now() - began} ms (dev build)`);
assert(back.includes("TAKE-ONE.mov"), "it now points at the file it found, under its new name");
assert(
  await evaluate(`[...document.querySelectorAll('.tree-row')].some((r) => r.textContent.includes('A001C001.mov'))`),
  "and the asset kept its identity and its name in the project",
);
await shot("relinked");

// The namesake left behind is a different take, and was not used.
const path = await evaluate(`(() => {
  const rows = [...document.querySelectorAll('dt')].find((d) => d.textContent === 'path');
  return rows ? rows.nextElementSibling.textContent : null;
})()`);
assert(path === moved, `the project points at the real file, not the namesake: ${path}`);

assert(pageErrors.length === 0, `no uncaught errors in the page (${pageErrors.join("; ")})`);
api.close();
cdp.close();
cleanup();
await sleep(300);
rmSync(work, { recursive: true, force: true });
console.log("relink e2e: all steps passed");
process.exit(0);
