// End-to-end drive of colour management against a running `cine start`.
//
// Needs a project with media placed on a sequence:
//
//   cine new /tmp/film && cine import /tmp/film ui/e2e/harbour.fountain
//   cine start /tmp/film --port 7395 --no-window
//   CINE_E2E_MEDIA=/a.mp4:/b.mp4 node ui/e2e/colour.mjs 7395
//
// Every media file must have a colorspace and say what decided it; choosing a
// view must reach a second client; a person's choice must stick, be undoable,
// and a colorspace the config does not have must be refused. Nothing mocked.
import { existsSync } from "node:fs";

const PORT = process.argv[2] ?? "7395";
const MEDIA = (process.env.CINE_E2E_MEDIA ??
  "/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4:/home/finstein-emp/Downloads/A_solitary_monochrome_crimson.mp4"
).split(":");

function client() {
  const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws`);
  let id = 1;
  const pending = new Map();
  const waiters = [];
  const views = [];
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.method === "view") {
      views.push(m.params);
      return;
    }
    if (m.method === "changed") {
      waiters.splice(0).forEach((w) => w());
      return;
    }
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
    views,
    call(method, params = {}) {
      const n = id++;
      ws.send(JSON.stringify({ jsonrpc: "2.0", id: n, method, params }));
      return new Promise((resolve, reject) => pending.set(n, { resolve, reject }));
    },
    nextChange: () => new Promise((r) => waiters.push(r)),
  };
}

const ok = (cond, msg) => {
  if (!cond) {
    console.error("FAIL:", msg);
    process.exit(1);
  }
  console.log("  ok ", msg);
};

const A = client();
const B = client();
await Promise.all([A.ready, B.ready]);

// Ingest media, so there is something with a colorspace to talk about.
if (!MEDIA.every((p) => existsSync(p))) {
  console.log("SKIP: set CINE_E2E_MEDIA to two decodable H.264 files");
  process.exit(0);
}
for (const path of MEDIA) await A.call("ingest", { path });

let t0 = performance.now();
const colour = await A.call("color");
const projectionMs = performance.now() - t0;
ok(
  (colour.config?.source ?? "").includes("cg-config-v4.0.0_aces-v2.0_ocio-v2.5"),
  `the project carries a colour config: ${colour.config?.name} (projection ${projectionMs.toFixed(1)} ms)`,
);
ok(colour.displays.length >= 5, `${colour.displays.length} displays offered`);
ok(
  colour.colorspaces.some((c) => c.name === "ACEScct"),
  `${colour.colorspaces.length} colorspaces to choose from`,
);
ok(colour.media.length > 0, `${colour.media.length} media file(s) with a colour identity`);
for (const m of colour.media) {
  ok(
    !!m.input_colorspace && (m.source ?? "").startsWith("rule:"),
    `${m.name}: ${m.input_colorspace} (${m.source})`,
  );
}

// Choosing a view reaches the other client without it asking.
t0 = performance.now();
const chosen = await A.call("color_view", {
  display: "Rec.1886 Rec.709 - Display",
  view: "ACES 2.0 - SDR 100 nits (Rec.709)",
});
const viewMs = performance.now() - t0;
ok(chosen.view.view === "ACES 2.0 - SDR 100 nits (Rec.709)", `view chosen (${viewMs.toFixed(1)} ms)`);
await new Promise((r) => setTimeout(r, 200));
ok(B.views.length > 0 && B.views.at(-1).view === chosen.view.view, "the other client was told");
ok((await B.call("color")).view.display === "Rec.1886 Rec.709 - Display", "and agrees");

let refused = null;
await A.call("color_view", { display: "sRGB - Display", view: "Kodak 2383" }).catch((e) => (refused = e.message));
ok(refused !== null, `a view the display does not have is refused: ${refused}`);

// A person overrules the file rule, and it can be undone.
const first = colour.media[0];
t0 = performance.now();
const set = await A.call("set_input_colorspace", { media: first.id, colorspace: "ACEScct" });
const setMs = performance.now() - t0;
const depts = set.impact.departments.map((d) => d.department);
ok(depts.length > 0, `the change reports its impact: ${depts.join(", ")} (${setMs.toFixed(1)} ms)`);
const after = (await B.call("color")).media.find((m) => m.id === first.id);
ok(after.input_colorspace === "ACEScct" && after.source === "user", "a person's choice, seen by both clients");

refused = null;
await A.call("set_input_colorspace", { media: first.id, colorspace: "Kodak Vision3" }).catch(
  (e) => (refused = e.message),
);
ok(refused !== null, `a colorspace the config lacks is refused: ${refused}`);

await A.call("undo");
const restored = (await B.call("color")).media.find((m) => m.id === first.id);
ok(
  restored.input_colorspace === first.input_colorspace && restored.source === first.source,
  `undo restores what the rule decided: ${restored.input_colorspace} (${restored.source})`,
);

console.log(
  `MEASURED colour projection ${projectionMs.toFixed(1)} ms; view chosen and broadcast ${viewMs.toFixed(1)} ms; ` +
    `input colorspace with cascade ${setMs.toFixed(1)} ms`,
);
console.log("ALL OK");
process.exit(0);
