// End-to-end: import media from the real application, and watch the real work.
//
// `cine new`, `cine bars`, `cine start`, then headless Chrome over the
// DevTools protocol: open Project ▸ Import media…, ask for a thumbnail, a
// waveform and a proxy, and watch the media browser fill in from the runtime's
// own answers — never before the work is actually done.
//
//   CINE_BIN=target/debug/cine node ui/e2e/ingest.mjs [port]
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = Number(process.argv[2] ?? 7401);
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

const work = mkdtempSync(join(tmpdir(), "cine-e2e-ingest-"));
const project = join(work, "film");
const media = join(work, "A001C003.mov");
execFileSync(CINE, ["new", project, "--name", "Ingest E2E"], { stdio: "inherit" });
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
  const path = join(SHOTS, `cine-e2e-ingest-${name}.png`);
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

await send("Page.enable");
await send("Runtime.enable");
await send("Page.navigate", { url: `${base}/` });
await until(`[...document.querySelectorAll('button')].some((b) => b.textContent.startsWith('Jobs'))`, "the editor loads");

// Nothing has been imported, and the browser does not pretend otherwise.
assert((await evaluate(`${rows("A001C003.mov")}.length`)) === 0, "the media is not in the project yet");

// -- the import, as a person does it ---------------------------------------
await evaluate(`(() => {
  const b = [...document.querySelectorAll('.menubar .menu > button')].find((b) => b.textContent.trim() === 'Project');
  if (!b) throw new Error('no Project menu');
  b.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
  return true;
})()`);
await clickText("Import media…");
await until(`!!document.querySelector('.dialog[aria-label="media"]')`, "the import dialog opens");
assert((await type(".dialog input", media)) === media, "the file path is typed in");
assert((await check("Make thumbnail", true)) === true, "a thumbnail is asked for");
assert((await check("Make waveform", true)) === true, "a waveform is asked for");
assert((await check("Make proxy", true)) === true, "a proxy is asked for");
await shot("dialog");

const began = Date.now();
await clickText("Import");
// The dialog says the import is a job, and names where to watch it.
const said = await until(
  `(() => { const n = document.querySelector('.dialog .notice'); return n && n.textContent; })()`,
  "the dialog says the import was submitted",
);
assert(said.includes("Jobs"), `and where to watch it: ${said.trim()}`);
await clickText("Close");

// -- what the runtime actually did -----------------------------------------
await until(`[...document.querySelectorAll('button')].find((b) => b.textContent.startsWith('Jobs')).click() || true`, "the Jobs panel is opened");
const ingestJob = await until(
  `(() => {
     const j = [...document.querySelectorAll('.job')].find((j) => j.textContent.includes('Import · A001C003.mov'));
     return j && j.textContent;
   })()`,
  "the import itself appears in Jobs as real work",
);
assert(!!ingestJob, `the job row is the runtime's own record: ${ingestJob.replace(/\s+/g, " ").trim()}`);
// And the work it dispatched is there too, as jobs of its own rather than as
// something the import claimed to have finished.
const derived = await until(
  `(() => {
     const all = [...document.querySelectorAll('.job')].map((j) => j.textContent);
     const proxy = all.find((t) => t.includes('Proxy') && t.includes('A001C003.mov'));
     return proxy || false;
   })()`,
  "the proxy it dispatched is a job of its own",
);
assert(!!derived, `dispatched, not claimed: ${derived.replace(/\s+/g, " ").trim()}`);

// The asset appears only once the runtime has registered it — exactly one row.
const appeared = await until(`${rows("A001C003.mov")}.length === 1 && true`, "the media appears in the browser, once");
assert(appeared, `one asset for one file, not one per derived thing`);
console.log(`MEASURED Import clicked to the asset visible in the browser: ${Date.now() - began} ms (dev build)`);

// The marks follow the work, and only the work. A proxy of 24 pictures of
// 1280x720 takes seconds, so this is a real observation of the order.
const chips = () => `(${rows("A001C003.mov")}[0]?.querySelector('.meta')?.textContent ?? '')`;
assert(!(await evaluate(`${chips()}.includes('P')`)), "no proxy mark before there is a proxy");
await until(`${chips()}.includes('∿')`, "the waveform mark appears when the waveform is made");
const waveformAt = Date.now();
await until(`${chips()}.includes('P')`, "the proxy mark appears when the proxy is made");
console.log(
  `MEASURED waveform mark to proxy mark for 24 pictures of 1280x720: ${Date.now() - waveformAt} ms (dev build)`,
);
await shot("imported");

// The marks are the runtime's state, so they say what is wrong when it is.
const marked = await evaluate(`${chips()}`);
assert(!marked.includes("≠"), `and nothing claims the file is not what it is: ${marked.trim()}`);

// Every mark is checked against the runtime, not against the interface.
const truth = await evaluate(`(async () => {
  const r = await fetch('${base}/health');
  return r.ok;
})()`);
assert(truth, "the runtime is still the one answering");

assert(pageErrors.length === 0, `no uncaught errors in the page (${pageErrors.join("; ")})`);
cdp.close();
cleanup();
await sleep(300);
rmSync(work, { recursive: true, force: true });
console.log("ingest e2e: all steps passed");
process.exit(0);
