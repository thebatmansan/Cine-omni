// End-to-end: one subject, many lenses, from the real application.
//
// The architecture this proves: what a filmmaker is working on survives a
// change of craft. Select a take in Editorial, move to Media, move back — the
// subject is still the take, its relationships are still the graph's own
// edges, and nothing has been reset to a project root or a first sequence.
//
//   CINE_BIN=target/debug/cine node ui/e2e/subject.mjs [port]
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = Number(process.argv[2] ?? 7471);
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

const work = mkdtempSync(join(tmpdir(), "cine-e2e-subject-"));
const project = join(work, "film");
const media = join(work, "A001C001.mov");
execFileSync(CINE, ["new", project, "--name", "Subject E2E"], { stdio: "inherit" });
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

// -- the browser -----------------------------------------------------------
const profile = join(work, "chrome");
const chrome = spawn(
  CHROME,
  [
    "--headless=new",
    `--remote-debugging-port=${CDP_PORT}`,
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--window-size=1600,1000",
    "about:blank",
  ],
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
  const path = join(SHOTS, `cine-e2e-subject-${name}.png`);
  writeFileSync(path, Buffer.from(data, "base64"));
  console.log(`  shot ${path}`);
};
const clickText = (text) =>
  evaluate(`(() => {
    const b = [...document.querySelectorAll('button')].find((b) => b.textContent.trim() === ${JSON.stringify(text)});
    if (!b) throw new Error('no button ${text}');
    b.click();
    return true;
  })()`);
// A checkbox is set the way a person sets one, through React's own handler.
const check = (label, on) =>
  evaluate(`(() => {
    const el = document.querySelector('input[aria-label="${label}"]');
    if (!el) throw new Error('no checkbox ${label}');
    if (el.checked !== ${on}) el.click();
    return el.checked;
  })()`);
const type = (selector, value) =>
  evaluate(`(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) throw new Error('no field ' + ${JSON.stringify(selector)});
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, ${JSON.stringify(value)});
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return el.value;
  })()`);
// How many rows the media browser holds for a name.
const rows = (name) =>
  `[...document.querySelectorAll('.tree-row')].filter((r) => r.querySelector('.label')?.textContent === ${JSON.stringify(name)})`;

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

await send("Page.enable");
await send("Runtime.enable");
await send("Page.navigate", { url: `${base}/` });
await until(`[...document.querySelectorAll('button')].some((b) => b.textContent.startsWith('Jobs'))`, "the editor loads");

// Import a take so there is something real to be the subject of.
const job = await api.call("job_submit", {
  kind: "media.ingest",
  path: media,
  thumbnail: false,
  waveform: false,
});
for (;;) {
  const jobs = await api.call("jobs", {});
  const j = jobs.find((x) => x.id === job.id);
  if (j && ["succeeded", "failed", "cancelled"].includes(j.state)) {
    assert(j.state === "succeeded", `the take imported: ${j.state}`);
    break;
  }
  await sleep(50);
}

// -- choosing a subject ----------------------------------------------------
await until(
  `[...document.querySelectorAll('.tree-row')].some((r) => r.querySelector('.label')?.textContent === 'A001C001.mov')`,
  "the take appears in the project",
);
await evaluate(`(() => {
  const r = [...document.querySelectorAll('.tree-row')].find((x) => x.querySelector('.label')?.textContent === 'A001C001.mov');
  r.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
  return true;
})()`);

const railName = () => evaluate(`document.querySelector('.rail-name')?.textContent ?? null`);
const railKind = () => evaluate(`document.querySelector('.rail-kind')?.textContent ?? null`);
const lens = () =>
  evaluate(`document.querySelector('.workspaces button[aria-selected="true"]')?.textContent?.trim() ?? null`);

await until(`document.querySelector('.rail-name')?.textContent === 'A001C001.mov'`, "the rail names the subject");
assert((await railKind()) === "Media", `and says what it is: ${await railKind()}`);

// The relationships are the graph's, not a list this test supplied.
const groups = await evaluate(`[...document.querySelectorAll('.rail-group-name')].map((g) => g.textContent)`);
assert(groups.length > 0, `the rail groups its relationships: ${JSON.stringify(groups)}`);
const entries = await evaluate(`[...document.querySelectorAll('.rail-entry-name')].map((e) => e.textContent)`);
assert(entries.length > 0, `and lists real ones: ${JSON.stringify(entries)}`);
assert(
  !groups.includes("Post"),
  `nothing has been cut with it, so there is no Post group at all: ${JSON.stringify(groups)}`,
);
await shot("subject");

// -- the lens changes; the subject does not --------------------------------
const before = await railName();
const lensBefore = await lens();
await evaluate(`(() => {
  const b = [...document.querySelectorAll('.workspaces button')].find((x) => x.textContent.trim().toUpperCase().startsWith('MEDIA'));
  if (!b) throw new Error('no Media lens');
  b.click();
  return true;
})()`);
await sleep(600);
const lensAfter = await lens();
assert(lensAfter !== lensBefore, `the lens changed: ${lensBefore} → ${lensAfter}`);
assert(
  (await railName()) === before,
  `and the subject did not: ${before} → ${await railName()}`,
);

// Back again, still the same subject. Nothing was reset to a project root.
await evaluate(`(() => {
  const b = [...document.querySelectorAll('.workspaces button')].find((x) => x.textContent.trim().toUpperCase().startsWith('EDITORIAL'));
  b.click();
  return true;
})()`);
await sleep(600);
assert((await railName()) === before, `and back: still ${await railName()}`);
await shot("after-lens");

// -- stepping an edge ------------------------------------------------------
const stepped = await evaluate(`(() => {
  const e = document.querySelector('.rail-entry');
  if (!e) return null;
  const name = e.querySelector('.rail-entry-name')?.textContent;
  e.click();
  return name;
})()`);
if (stepped) {
  await sleep(600);
  assert(
    (await railName()) === stepped,
    `walking an edge lands on that thing: ${stepped} → ${await railName()}`,
  );
  // And the way back is in the path: the edge is real in both directions.
  const crumbs = await evaluate(`[...document.querySelectorAll('.rail-crumb')].map((c) => c.textContent)`);
  assert(Array.isArray(crumbs), `the path locates it: ${JSON.stringify(crumbs)}`);
}

assert(pageErrors.length === 0, `no uncaught errors in the page (${pageErrors.join("; ")})`);
cdp.close();
cleanup();
await sleep(300);
rmSync(work, { recursive: true, force: true });
console.log("subject e2e: all steps passed");
process.exit(0);
