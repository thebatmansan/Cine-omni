// End-to-end: the production office against a running `cine start`.
//
//   cine new /tmp/film && cine import /tmp/film ui/e2e/harbour.fountain
//   cine start /tmp/film --port 7394 --no-window
//   node ui/e2e/production.mjs 7394
//
// A script change that lengthens a scene must reach the stripboard: the
// scene's page count grows, the schedule reflects it, and a second client sees
// the same schedule without asking twice. Every step is asserted and timed.
const PORT = process.argv[2] ?? "7394";

function client() {
  const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws`);
  let id = 1;
  const pending = new Map();
  const waiters = [];
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
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

let t0 = performance.now();
const before = await A.call("production");
const projectionMs = performance.now() - t0;
ok(before.breakdowns.length >= 2, `breakdown for ${before.breakdowns.length} scenes (projection ${projectionMs.toFixed(1)} ms)`);
ok(before.schedule && before.schedule.days.length >= 1, `stripboard of ${before.schedule.days.length} day(s)`);
ok(before.call_sheets.length === before.schedule.days.length, "a call sheet per day");

const scene1 = before.breakdowns[0];
const script = await A.call("script");
const line = script.documents[0].scenes[0].elements.find((e) => e.kind === "dialogue");
const monologue = Array(40).fill("We ride at dawn and we do not stop until the river is behind us.").join(" ");

const seen = B.nextChange();
t0 = performance.now();
const edit = await A.call("edit_dialogue", { line: line.id, text: monologue });
const editMs = performance.now() - t0;
const depts = edit.impact.departments.map((d) => d.department);
ok(depts.includes("Script") && depts.includes("Production"), `the rewrite reached production: ${depts.join(", ")} (${editMs.toFixed(1)} ms)`);
await seen;

const after = await B.call("production");
const grown = after.breakdowns.find((b) => b.scene === scene1.scene);
ok(grown.page_eighths > scene1.page_eighths, `scene ${grown.number} grew from ${scene1.page_eighths}/8 to ${grown.page_eighths}/8 pages`);
const strip = after.schedule.days.flatMap((d) => d.strips).find((s) => s.scene === scene1.scene);
ok(strip.pages_eighths === grown.page_eighths, "the stripboard carries the new length");
const sheetScene = after.call_sheets.flatMap((c) => c.scenes).find((s) => s.scene === scene1.scene);
ok(sheetScene != null, `the call sheet lists it (${sheetScene?.pages} pages)`);

await A.call("undo");
const restored = await B.call("production");
ok(
  restored.breakdowns.find((b) => b.scene === scene1.scene).page_eighths === scene1.page_eighths,
  "undo restores the page count everywhere",
);
// The board is a person's: move a strip, and it stays where it was put.
const dayOf = (p, scene) =>
  p.schedule.days.find((d) => d.strips.some((s) => s.scene === scene))?.index ?? null;
const board = await A.call("production");
const wasOn = dayOf(board, scene1.scene);
t0 = performance.now();
const moved = await A.call("move_strip", {
  document: board.document,
  scene: scene1.scene,
  day: wasOn + 1,
});
const moveMs = performance.now() - t0;
ok(moved.impact.departments.length > 0, `moving a strip is a change with consequences (${moveMs.toFixed(1)} ms)`);
const boarded = await B.call("production");
ok(dayOf(boarded, scene1.scene) === wasOn + 1, `scene ${scene1.number} stayed on day ${wasOn + 1}`);
ok(
  Object.keys(boarded.constraints.pinned ?? {}).includes(scene1.scene),
  "and it is a constraint, so a reschedule keeps it",
);
const allScenes = boarded.schedule.days.flatMap((d) => d.strips.map((s) => s.scene));
ok(allScenes.length === boarded.breakdowns.length, "nothing was lost or duplicated by the move");
const dood = boarded.dood;
ok(dood && dood.cast.length > 0, `day out of days: ${dood.cast.length} cast, ${dood.total_held} hold day(s)`);
await A.call("move_strip", { document: boarded.document, scene: scene1.scene, day: null });
ok(dayOf(await B.call("production"), scene1.scene) === wasOn, "released, the scheduler decides again");


console.log(
  `MEASURED production projection ${projectionMs.toFixed(1)} ms; dialogue rewrite with full cascade ${editMs.toFixed(1)} ms; ` +
    `strip move with reschedule ${moveMs.toFixed(1)} ms`,
);
console.log("ALL OK");
process.exit(0);
