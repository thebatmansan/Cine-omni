// End-to-end: make a proxy and play it, from the real application.
//
// `cine new`, `cine bars`, `cine start`, then headless Chrome over the
// DevTools protocol: select the media, make a proxy, watch the job finish,
// switch playback to proxies, see the record monitor say so, then break the
// proxy file and watch the application stop trusting it.
//
//   CINE_BIN=target/debug/cine node ui/e2e/proxy.mjs [port]
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = Number(process.argv[2] ?? 7397);
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

const work = mkdtempSync(join(tmpdir(), "cine-e2e-proxy-"));
const project = join(work, "film");
const media = join(work, "bars.mov");
execFileSync(CINE, ["new", project, "--name", "Proxy E2E"], { stdio: "inherit" });
execFileSync(CINE, ["bars", media, "--seconds", "1", "--size", "1280x720"], { stdio: "inherit" });

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
assert(true, "a one-second 1280x720 cut is open");

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
  const path = join(SHOTS, `cine-e2e-proxy-${name}.png`);
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

// Select the media in the browser: the inspector then shows it.
await until(
  `(() => {
     const row = [...document.querySelectorAll('.tree-row')].find((r) => r.querySelector('.label')?.textContent === 'bars.mov');
     if (!row) return false;
     // The browser selects on mousedown, as a person's click does.
     row.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
     return true;
   })()`,
  "the media can be selected in the project browser",
);
await until(`document.body.textContent.includes('Proxies')`, "the inspector shows the media's proxies");
assert(
  await evaluate(`document.body.textContent.includes('No proxy of this original')`),
  "it says there is no proxy yet",
);

// Make a proxy of it, and watch the job.
assert((await choose("Proxy height", "360")) === "360", "360p chosen");
const began = Date.now();
await clickText("Make proxy");
const row = await until(
  `(() => { const r = document.querySelector('.proxy-row.current'); return r && r.textContent; })()`,
  "the proxy appears as current",
);
console.log(`MEASURED submit to a current 640x360 proxy of 24 pictures of 1280x720, visible in the browser: ${Date.now() - began} ms (dev build)`);
assert(row.includes("640×360") && row.includes("sound"), `the proxy row describes it: ${row}`);
await shot("made");

// Play proxies: the record monitor says which file it is showing.
await choose("Media", "proxy");
// The switch shows what the runtime has, so it settles when the runtime answers.
await until(`document.querySelector('select[aria-label="Media"]').value === 'proxy'`, "playback is set to proxies everywhere");
const note = await until(
  `(() => { const n = document.querySelector('.viewer.record .viewer-note'); return n && n.textContent.includes('PROXY') && n.textContent; })()`,
  "the record monitor says it is showing a proxy",
);
assert(!!note, `the monitor is explicit: ${note.trim()}`);
const src = await evaluate(`document.querySelector('.viewer.record img')?.src ?? ""`);
assert(src.startsWith("blob:"), "and it is a decoded picture, not a placeholder");
await shot("playing-proxy");

// Break the proxy file: the application stops trusting it.
const proxyPath = await evaluate(`(() => {
  const el = [...document.querySelectorAll('.proxy-row .label')][0];
  return el ? el.getAttribute('title') : null;
})()`);
assert(!!proxyPath, `the proxy's path is shown: ${proxyPath}`);
writeFileSync(proxyPath, Buffer.concat([readFileSync(proxyPath), Buffer.alloc(64)]));
await clickText("Check media");
const stale = await until(
  `(() => { const r = document.querySelector('.proxy-row.stale'); return r && r.textContent; })()`,
  "the proxy is shown as stale once its file changed",
);
assert(stale.includes("changed on disk"), `and says why: ${stale}`);
await shot("stale");

assert(pageErrors.length === 0, `no uncaught errors in the page (${pageErrors.join("; ")})`);
api.close();
cdp.close();
cleanup();
await sleep(300);
rmSync(work, { recursive: true, force: true });
console.log("proxy e2e: all steps passed");
process.exit(0);
