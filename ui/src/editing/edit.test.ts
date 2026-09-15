import { describe, expect, it } from "vitest";
import type { Time, TimelineItem, TimelineTrack } from "../rpc/client";
import { editFor, frameAt, framesIn, leftOfCut, snap, frameLabel } from "./edit";

const R24 = { num: 24, den: 1 };
const NTSC = { num: 24000, den: 1001 };

function t(frames: number, rate = R24): Time {
  return {
    ticks: frames,
    rate_num: rate.num,
    rate_den: rate.den,
    seconds: (frames * rate.den) / rate.num,
    timecode: "",
  };
}

function item(index: number, start: number, dur: number, kind: TimelineItem["kind"] = "clip"): TimelineItem {
  return {
    index,
    id: `i${index}`,
    kind,
    name: `item ${index}`,
    at: { start: t(start), duration: t(dur), end: t(start + dur) },
    source_range: null,
    enabled: true,
    media: null,
    available_range: null,
    handles: null,
    link: null,
    needs_conform: false,
    conform_reason: null,
    scenes: [],
    asset: null,
  };
}

const track = (items: TimelineItem[]): TimelineTrack => ({
  index: 0,
  id: "v1",
  name: "V1",
  kind: "video",
  enabled: true,
  locked: false,
  muted: false,
  solo: false,
  targeted: false,
  duration: t(0),
  items,
  markers: [],
});

describe("pixels to frames", () => {
  it("a click lands in the frame it falls in", () => {
    expect(frameAt(1.0, R24)).toBe(24);
    expect(frameAt(1.04, R24)).toBe(24);
    expect(frameAt(1.0 + 1 / 24, R24)).toBe(25);
  });

  it("an NTSC position is counted in NTSC frames, not 24ths of a second", () => {
    // 1001 seconds of 23.976 is exactly 24000 frames.
    expect(frameAt(1001, NTSC)).toBe(24000);
    expect(framesIn(1001 / 24000, NTSC)).toBe(1);
  });

  it("a drag rounds to the nearest whole frame", () => {
    expect(framesIn(0.49 / 24, R24)).toBe(0);
    expect(framesIn(0.51 / 24, R24)).toBe(1);
    expect(framesIn(-12.4 / 24, R24)).toBe(-12);
  });
});

describe("snapping", () => {
  it("snaps within tolerance to the nearest target, never to the dragged edge itself", () => {
    const targets = [0, 5, 5.1];
    expect(snap(5.04, targets, 100)).toEqual({ position: 5, snapped: true });
    expect(snap(5.2, targets, 100)).toEqual({ position: 5.2, snapped: false });
    expect(snap(5.04, targets, 100, 8, 5)).toEqual({ position: 5.1, snapped: true });
  });
});

describe("gesture to edit", () => {
  const items = [item(0, 0, 48), item(1, 36, 24, "transition"), item(2, 48, 72), item(3, 120, 24, "gap")];
  const tr = track(items);

  it("dragging a tail with the select tool is a ripple trim in frames", () => {
    expect(editFor("select", { kind: "edge", edge: "tail", dxSeconds: 0.5 }, items[0], tr, R24)).toEqual({
      op: "ripple_trim",
      index: 0,
      edge: "tail",
      delta: { ticks: 12, rate_num: 24, rate_den: 1 },
    });
  });

  it("a zero-frame drag asks for nothing", () => {
    expect(editFor("ripple", { kind: "edge", edge: "head", dxSeconds: 0.01 }, items[0], tr, R24)).toBeNull();
  });

  it("rolling a head rolls the cut on its left, across a transition", () => {
    expect(leftOfCut(tr, 2)).toBe(0);
    expect(editFor("roll", { kind: "edge", edge: "head", dxSeconds: -1 }, items[2], tr, R24)).toEqual({
      op: "roll",
      cut: 0,
      delta: { ticks: -24, rate_num: 24, rate_den: 1 },
    });
    expect(editFor("roll", { kind: "edge", edge: "head", dxSeconds: 1 }, items[0], tr, R24)).toBeNull();
  });

  it("slip moves the content with the hand, slide moves the clip", () => {
    expect(editFor("slip", { kind: "body", dxSeconds: 1 }, items[2], tr, R24)).toMatchObject({
      op: "slip",
      delta: { ticks: -24 },
    });
    expect(editFor("slide", { kind: "body", dxSeconds: 1 }, items[2], tr, R24)).toMatchObject({
      op: "slide",
      delta: { ticks: 24 },
    });
    expect(editFor("slip", { kind: "body", dxSeconds: 1 }, items[3], tr, R24)).toBeNull();
  });

  it("the razor splits inside a clip and refuses a cut that already exists", () => {
    expect(editFor("razor", { kind: "click", atSeconds: 3 }, items[2], tr, R24)).toEqual({
      op: "split_at",
      time: { ticks: 72, rate_num: 24, rate_den: 1 },
    });
    expect(editFor("razor", { kind: "click", atSeconds: 2 }, items[2], tr, R24)).toBeNull();
    expect(editFor("razor", { kind: "click", atSeconds: 1 }, items[1], tr, R24)).toBeNull();
  });

  it("labels deltas as frames", () => {
    expect(frameLabel(12)).toBe("+12f");
    expect(frameLabel(-3)).toBe("−3f");
  });
});
