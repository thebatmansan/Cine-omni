// End-to-end drive of the editor slice against a running `cine start`.
//
// Two clients over real WebSockets; every step asserted; latencies measured.
// It needs a project with the two-scene screenplay imported and two decodable
// H.264 files:
//
//   cine new /tmp/film && cine import /tmp/film harbour.fountain
//   cine start /tmp/film --port 7391
//   CINE_E2E_MEDIA=/a.mp4:/b.mp4 node ui/e2e/editor.mjs 7391
//
// The screenplay's first scene must contain a dialogue line. Nothing here is
// mocked: the edits land in the project's op log.
import { existsSync } from "node:fs";

const PORT = process.argv[2] ?? "7391";
const MEDIA = (process.env.CINE_E2E_MEDIA ??
  "/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4:/home/finstein-emp/Downloads/A_solitary_monochrome_crimson.mp4"
).split(":");
if (!MEDIA.every((p) => existsSync(p))) {
  console.log("SKIP: set CINE_E2E_MEDIA to two decodable H.264 files");
  process.exit(2);
}

function client(name) {
  const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws`);
  let id = 1;
  const pending = new Map();
  const changes = [];
  const waiters = [];
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.method === "changed") {
      changes.push({ at: performance.now(), ...m.params });
      waiters.splice(0).forEach((w) => w());
      return;
    }
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    m.error ? p.reject(new Error(m.error.message)) : p.resolve(m.result);
  };
  const ready = new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  return {
    name, ready, changes,
    call(method, params = {}) {
      const n = id++;
      ws.send(JSON.stringify({ jsonrpc: "2.0", id: n, method, params }));
      return new Promise((resolve, reject) => pending.set(n, { resolve, reject }));
    },
    nextChange() { return new Promise((r) => waiters.push(r)); },
    close() { ws.close(); },
  };
}

const assert = (cond, msg) => { if (!cond) { console.error("FAIL:", msg); process.exit(1); } console.log("  ok ", msg); };
const f24 = (n) => ({ ticks: n, rate_num: 24, rate_den: 1 });
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))]; };

const A = client("A"), B = client("B");
await Promise.all([A.ready, B.ready]);

console.log("1. a sequence and real media");
const seq = await A.call("new_timeline", { name: "Assembly", rate_num: 24, rate_den: 1 });
const tl = seq.id;
let t = await A.call("timeline", { id: tl });
const [v1, a1] = [t.tracks[0].id, t.tracks[1].id];
assert(t.tracks.length === 2 && t.tracks[0].kind === "video", "new sequence has V1 and A1");
const assets = [];
for (const path of MEDIA) assets.push((await A.call("ingest", { path })).id);
const media = await A.call("media");
const info = assets.map((id) => media.find((m) => m.id === id));
assert(info.every((m) => m.video?.decodable), "both files ingested and decodable");

console.log("2. placing takes (picture + sound, linked)");
let at = 0;
for (const m of info) {
  const r = { num: m.video.rate_num, den: m.video.rate_den };
  const hasSound = m.streams.some((s) => s.name.startsWith("audio"));
  const tracks = hasSound ? [v1, a1] : [v1];
  const out = await A.call("place", {
    tracks, time: f24(at), mode: "insert",
    clip: { media: m.id, source_range: { start_time: { ticks: 10, rate_num: r.num, rate_den: r.den }, duration: { ticks: 48, rate_num: r.num, rate_den: r.den } } },
  });
  assert(out.clips.length === tracks.length, `placed ${m.name} on ${tracks.length} track(s)`);
  at += Math.round((48 * r.den / r.num) * 24);
}
t = await A.call("timeline", { id: tl });
const clips = t.tracks[0].items.filter((i) => i.kind === "clip");
assert(clips.length === 2 && clips[1].at.start.ticks > 0, `V1 holds two clips end to end (second at ${clips[1].at.start.timecode})`);

console.log("3. record monitor resolves through the runtime");
const pic = await A.call("picture_at", { timeline: tl, time: f24(30) });
assert(!pic.black && pic.asset === clips[0].asset && pic.index != null, `frame 30 of the cut is picture #${pic.index} of ${pic.name}`);
let t0 = performance.now();
const png = await fetch(`http://127.0.0.1:${PORT}/frame?media=${pic.asset}&index=${pic.index}`);
const buf = new Uint8Array(await png.arrayBuffer());
assert(png.status === 200 && buf[0] === 0x89, `record picture decodes to PNG (${buf.length} bytes, ${(performance.now() - t0).toFixed(0)} ms, decode ${png.headers.get("x-cine-decode-ms")} ms)`);

console.log("4. bind cut material to scene 1, change the script, see the impact");
const script = await A.call("script");
const scene = script.documents[0].scenes[0];
const line = scene.elements.find((e) => e.kind === "dialogue");
await A.call("bind_clip", { clip: clips[0].id, scene: scene.id });
const bChange = B.nextChange();
const edit = await A.call("edit_dialogue", { line: line.id, text: "We sail at noon." });
assert(edit.impact.conform.length === 1 && edit.impact.conform[0].clip === clips[0].id, `impact names the bound clip: "${edit.impact.conform[0]?.reason}"`);
assert(edit.impact.departments.map((d) => d.department).join(",").includes("Script"), `departments: ${edit.impact.departments.map((d) => `${d.department}(${d.count})`).join(" ")}`);
await bChange;
await new Promise((r) => setTimeout(r, 50));
const bImpact = await B.call("impact", { revision: B.changes.at(-1).revision });
assert(bImpact.label === "edit dialogue", "the other client reads the same impact by revision");
const bt = await B.call("timeline", { id: tl });
assert(bt.tracks[0].items.find((i) => i.id === clips[0].id).needs_conform === true, "the other client sees the clip flagged");
assert(bt.tracks[0].items.find((i) => i.id === clips[1].id).needs_conform === false, "the unbound clip is untouched");

console.log("5. linked edits, razor, lift, extract, undo from the other client");
const before = await A.call("timeline", { id: tl });
const trim = await A.call("edit", { track: v1, op: { op: "ripple_trim", index: 0, edge: "tail", delta: f24(-6) } });
assert(trim.outcome.applied.ticks === -6, `trim applied ${trim.outcome.applied.ticks}f, linked ${trim.outcome.linked_clips}`);
const split = await A.call("edit", { track: v1, op: { op: "split_at", time: f24(20) } });
assert(split.outcome.changed_tracks.length >= 1, `split changed ${split.outcome.changed_tracks.length} track(s)`);
const afterSplit = await A.call("timeline", { id: tl });
const piece = afterSplit.tracks[0].items.find((i) => i.kind === "clip" && i.at.start.ticks === 20);
const lift = await A.call("edit", { track: v1, op: { op: "lift", range: { start_time: f24(20), duration: { ticks: piece.at.duration.ticks, rate_num: 24, rate_den: 1 } } } });
assert(lift.outcome.label === "lift", "lift committed");
const lifted = await A.call("timeline", { id: tl });
assert(lifted.tracks[0].duration.ticks === afterSplit.tracks[0].duration.ticks, "lift leaves the sequence length alone");
for (let i = 0; i < 3; i++) await B.call("undo");
const restored = await A.call("timeline", { id: tl });
assert(JSON.stringify(restored.tracks) === JSON.stringify(before.tracks), "three undos from B restore A's cut exactly");
const history = await B.call("history");
assert(history.redo_depth === 3, `history shows ${history.undo_depth} undo, ${history.redo_depth} redo`);

console.log("6. measured: edit → commit → broadcast → the other client refetched");
const roundTrip = [], propagate = [];
for (let i = 0; i < 40; i++) {
  const delta = i % 2 === 0 ? -1 : 1;
  const seen = B.nextChange();
  const s = performance.now();
  await A.call("edit", { track: v1, op: { op: "ripple_trim", index: 0, edge: "tail", delta: f24(delta) } });
  roundTrip.push(performance.now() - s);
  await seen;
  await Promise.all([B.call("project"), B.call("timeline", { id: tl }), B.call("script"), B.call("history"), B.call("media"), B.call("structure"), B.call("timelines")]);
  propagate.push(performance.now() - s);
}
console.log(`MEASURED edit reply (A):            median ${pct(roundTrip, 50).toFixed(1)} ms, p95 ${pct(roundTrip, 95).toFixed(1)} ms, max ${Math.max(...roundTrip).toFixed(1)} ms (n=40)`);
console.log(`MEASURED edit → B refetched all:    median ${pct(propagate, 50).toFixed(1)} ms, p95 ${pct(propagate, 95).toFixed(1)} ms, max ${Math.max(...propagate).toFixed(1)} ms (n=40)`);
const proj = await A.call("project");
console.log(`project now rev ${proj.revision}, ${proj.nodes} nodes, ${proj.edges} edges, op log ${proj.log_len}`);
A.close(); B.close();
console.log("ALL OK");
process.exit(0);
