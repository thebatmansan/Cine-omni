// End-to-end: render a movie from the real application, in a real browser.
//
// Everything here is the product, not a stand-in: `cine new` makes a project,
// `cine bars` writes its media, `cine start` runs the runtime and serves the
// UI, and headless Chrome — driven over the DevTools protocol — opens the
// editor, chooses a movie render in the Jobs tab, submits it, and watches the
// render appear, preview, download, be refused, and go stale. Only the cut is
// assembled over the protocol, because assembling a cut by clicks is not what
// is being tested.
//
//   CINE_BIN=target/debug/cine node ui/e2e/render.mjs [port]
//
// Screenshots go to $CINE_E2E_SHOTS (default: the system temp directory).
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = Number(process.argv[2] ?? 7393);
const CDP_PORT = PORT + 1000;
const CINE = process.env.CINE_BIN ?? "target/debug/cine";
const SHOTS = process.env.CINE_E2E_SHOTS ?? tmpdir();
const CHROME = process.env.CHROME_BIN ?? "google-chrome";
if (!existsSync(CINE)) {
  console.log(`SKIP: no cine binary at ${CINE}; build it or set CINE_BIN`);
  process.exit(2);
}

const assert = (cond, msg) => {
  if (!cond) {
    console.error("FAIL:", msg);
    cleanup();
    process.exit(1);
  }
  console.log("  ok ", msg);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const work = mkdtempSync(join(tmpdir(), "cine-e2e-render-"));
const project = join(work, "film");
const media = join(work, "bars.mov");
const children = [];
function cleanup() {
  for (const c of children) {
    try {
      c.kill("SIGTERM");
    } catch {}
  }
}
process.on("exit", cleanup);

console.log("project and media");
execFileSync(CINE, ["new", project, "--name", "E2E"], { stdio: "inherit" });
execFileSync(CINE, ["bars", media, "--seconds", "4", "--size", "320x180"], { stdio: "inherit" });

const server = spawn(CINE, ["start", project, "--port", String(PORT), "--no-window"], { stdio: ["ignore", "inherit", "inherit"] });
children.push(server);
const base = `http://127.0.0.1:${PORT}`;
for (let i = 0; ; i++) {
  try {
    const r = await fetch(`${base}/health`);
    if (r.ok) break;
  } catch {}
  if (i > 300) throw new Error("cine start did not come up");
  await sleep(100);
}
assert(true, "cine start is serving");

// -- the cut, over the protocol --------------------------------------------
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
const seq = await api.call("new_timeline", { name: "Screener", rate_num: 24, rate_den: 1 });
const t = await api.call("timeline", { id: seq.id });
const [v1, a1] = [t.tracks[0].id, t.tracks[1].id];
const asset = (await api.call("ingest", { path: media })).id;
await api.call("place", {
  tracks: [v1, a1],
  time: f24(0),
  clip: { media: asset, source_range: { start_time: f24(0), duration: f24(48) } },
});
await api.call("focus", { timeline: seq.id });
assert(true, "a two-second cut of bars and pops is open");

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
const consoleErrors = [];
cdp.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === "Runtime.exceptionThrown") consoleErrors.push(m.params.exceptionDetails.text);
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
const until = async (expression, what, ms = 120000) => {
  const t0 = Date.now();
  for (;;) {
    const v = await evaluate(expression).catch(() => null);
    if (v) return v;
    if (Date.now() - t0 > ms) {
      await shot(`timeout-${what.replace(/\W+/g, "-")}`);
      assert(false, `${what} (timed out after ${ms} ms)`);
    }
    await sleep(100);
  }
};
const shot = async (name) => {
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  const path = join(SHOTS, `cine-e2e-render-${name}.png`);
  writeFileSync(path, Buffer.from(data, "base64"));
  console.log(`  shot ${path}`);
};
// React listens for the events a person's input makes, so set values the way
// the browser does and fire them.
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
const clickButton = (scope, text) =>
  evaluate(`(() => {
    const root = ${scope};
    const b = [...root.querySelectorAll('button')].find((b) => b.textContent.trim().startsWith(${JSON.stringify(text)}));
    if (!b) throw new Error('no button ${text}');
    b.click();
    return true;
  })()`);

await send("Page.enable");
await send("Runtime.enable");
await send("Page.navigate", { url: `${base}/` });
await until(`[...document.querySelectorAll('button')].some((b) => b.textContent.startsWith('Jobs'))`, "the editor loads");
await clickButton("document", "Jobs");
await until(`document.querySelector('[aria-label="Render"]')?.textContent.includes('Screener')`, "the Jobs tab offers to render the open sequence");
assert(true, "the Render form names the open sequence");

// A movie, with the cut's sound.
assert((await choose("Output", "movie")) === "movie", "movie chosen");
await until(`!!document.querySelector('select[aria-label="Sound"]')`, "a movie offers sound");
const submitted = Date.now();
await clickButton(`document.querySelector('[aria-label="Render"]')`, "Render");
const row = await until(
  `(() => { const r = document.querySelector('.render-row.current'); return r && r.textContent; })()`,
  "the render appears as current",
);
console.log(`MEASURED submit to a current movie render visible in the browser: ${Date.now() - submitted} ms (dev build, 48 frames 320x180 with sound)`);
assert(row.includes("movie") && row.includes("48 frames") && row.includes("320×180") && row.includes("sound"), `the render row describes it: ${row}`);
const jobRow = await evaluate(`[...document.querySelectorAll('.job.succeeded')].map((j) => j.textContent).find((t) => t.includes('Render movie'))`);
assert(!!jobRow, `the job shows as succeeded: ${jobRow}`);

await clickButton(`document.querySelector('.render-row.current')`, "Preview");
const previewWidth = await until(
  `(() => { const i = document.querySelector('.render-preview'); return i && i.complete && i.naturalWidth; })()`,
  "the preview picture decodes from the movie",
);
assert(previewWidth === 320, `the preview is the movie's own picture, ${previewWidth} wide`);
const href = await evaluate(`[...document.querySelectorAll('.render-row.current a')].find((a) => a.textContent === 'Movie').getAttribute('href')`);
const movie = await fetch(`${base}${href}`);
const bytes = new Uint8Array(await movie.arrayBuffer());
assert(movie.status === 200 && movie.headers.get("content-type") === "video/quicktime", "the movie link serves the movie");
assert(new TextDecoder().decode(bytes.slice(4, 8)) === "ftyp" && bytes.length > 10000, `it is a QuickTime file of ${bytes.length} bytes`);
await shot("current");

// A render the runtime refuses: a different shape with no fit chosen.
await type("Size", "320x320");
await until(`!!document.querySelector('select[aria-label="Fit"]')`, "a size offers a fit");
await clickButton(`document.querySelector('[aria-label="Render"]')`, "Render");
const refusal = await until(
  `(() => { const r = [...document.querySelectorAll('.render-row.failed')].map((r) => r.textContent).find((t) => t.includes('letterbox')); return r; })()`,
  "the refusal is shown with its reason",
);
assert(refusal.includes("different shape"), `refused, and says why: ${refusal}`);
await shot("refused");

// The cut changes where the render reads it: the render goes stale, and says what changed.
await api.call("edit", { track: v1, op: { op: "ripple_trim", index: 0, edge: "head", delta: f24(6) } });
const stale = await until(
  `(() => { const r = document.querySelector('.render-row.stale'); return r && r.textContent; })()`,
  "the render goes stale after the cut changes",
);
assert(stale.includes("changed"), `stale, with the reason: ${stale}`);
await shot("stale");

// Undo, and it is current again: the same inputs.
await api.call("undo", {});
await until(`!!document.querySelector('.render-row.current')`, "undo makes it current again");
assert(true, "undo makes the render current again");

assert(consoleErrors.length === 0, `no uncaught errors in the page (${consoleErrors.join("; ")})`);
api.close();
cdp.close();
cleanup();
await sleep(300);
rmSync(work, { recursive: true, force: true });
console.log("render e2e: all steps passed");
process.exit(0);
