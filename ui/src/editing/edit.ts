import type {
  EditOp,
  ExactRange,
  ExactTime,
  Time,
  TimelineItem,
  TimelineTrack,
  TimelineView,
} from "../rpc/client";

/**
 * Turning a gesture into an edit.
 *
 * The browser decides *which frame* a gesture means and *which operation* it
 * asks for. It never decides what the cut becomes: the runtime applies the edit
 * with exact arithmetic, clamps it to real media, carries it to linked
 * material, and the next projection is the answer. A float appears only on the
 * way from a pixel to a frame number, and is rounded to a whole frame before it
 * leaves this file.
 */

export type Tool = "select" | "ripple" | "roll" | "slip" | "slide" | "razor";

export const TOOLS: { id: Tool; key: string; label: string; help: string }[] = [
  { id: "select", key: "a", label: "Select", help: "Select clips; drag an edge to ripple-trim" },
  { id: "ripple", key: "t", label: "Ripple", help: "Drag an edge: trim it, and everything after moves" },
  { id: "roll", key: "r", label: "Roll", help: "Drag a cut: both sides move, the sequence length does not" },
  { id: "slip", key: "y", label: "Slip", help: "Drag a clip: change which media it plays, not where" },
  { id: "slide", key: "u", label: "Slide", help: "Drag a clip: move it between its neighbours" },
  { id: "razor", key: "b", label: "Razor", help: "Click: split the clip at that frame" },
];

export interface Rate {
  num: number;
  den: number;
}

export function rateOf(t: Time | ExactTime): Rate {
  return { num: t.rate_num, den: t.rate_den };
}

/** The sequence's own rate: its declared start, else what its cut is counted in. */
export function sequenceRate(timeline: TimelineView): Rate {
  return rateOf(timeline.global_start_time ?? timeline.duration);
}

export function exact(ticks: number, rate: Rate): ExactTime {
  return { ticks, rate_num: rate.num, rate_den: rate.den };
}

/** Display seconds of an exact time. For layout only. */
export function seconds(t: ExactTime): number {
  return (t.ticks * t.rate_den) / t.rate_num;
}

/** The frame a position in seconds falls in (floor): where a click lands. */
export function frameAt(s: number, rate: Rate): number {
  return Math.floor((s * rate.num) / rate.den + 1e-9);
}

/** The nearest whole number of frames to a distance in seconds: what a drag means. */
export function framesIn(s: number, rate: Rate): number {
  return Math.round((s * rate.num) / rate.den);
}

/**
 * Whether a clip offers trim handles.
 *
 * A retimed clip does not: the runtime refuses trims, rolls and joins on one
 * rather than guess whether the editor meant to keep the speed or the media,
 * and a grip that always fails is worse than no grip. Transitions and locked
 * tracks are the older reasons.
 */
export function canTrim(item: TimelineItem, track: TimelineTrack, tool: Tool): boolean {
  if (item.kind === "transition" || track.locked) return false;
  if (item.speed) return false;
  return tool === "select" || tool === "ripple" || tool === "roll";
}

/**
 * Where a new caption goes, in ticks at the sequence rate.
 *
 * The record marks when there are any: the in mark, and one past the picture
 * the out mark is on. Otherwise two seconds from the playhead. This proposes
 * a span; the runtime decides whether it is a caption, and refuses one that
 * ends before it starts.
 */
export function newCaptionSpan(
  marks: { in: number | null; out: number | null },
  playhead: number,
  rate: Rate,
): { start: number; end: number } {
  const start = marks.in ?? playhead;
  const end = marks.out != null ? marks.out + 1 : start + Math.round((2 * rate.num) / rate.den);
  return { start, end };
}

/** A pair of marks in whole frames at some rate. `null` is unmarked. */
export interface FrameMarks {
  in: number | null;
  out: number | null;
}

/**
 * Marks turned into seconds for the band that draws them.
 *
 * The out mark is *on* a picture, and that picture is part of the edit, so
 * the band runs to the end of it. Getting this wrong by one frame is the
 * classic off-by-one of every editing interface: the band would stop short of
 * the frame the editor marked and the edit would still include it.
 */
export function markSecondsOf(marks: FrameMarks, rate: Rate): { in: number | null; out: number | null } {
  const at = (frames: number | null) => (frames == null ? null : (frames * rate.den) / rate.num);
  return { in: at(marks.in), out: at(marks.out == null ? null : marks.out + 1) };
}

/** Every cut point on screen, in seconds, for snapping. */
export function snapTargets(timeline: TimelineView, playhead?: number): number[] {
  const out = new Set<number>();
  for (const track of timeline.tracks) {
    for (const item of track.items) {
      if (item.kind === "transition") continue;
      out.add(item.at.start.seconds);
      out.add(item.at.end.seconds);
    }
  }
  if (playhead != null) out.add(playhead);
  return [...out].sort((a, b) => a - b);
}

/**
 * Snap a position to the nearest target within `tolerancePx`, excluding
 * targets equal to `exclude` (the edge being dragged must not snap to itself).
 */
export function snap(
  position: number,
  targets: number[],
  pxPerSecond: number,
  tolerancePx = 8,
  exclude?: number,
): { position: number; snapped: boolean } {
  let best: number | null = null;
  for (const t of targets) {
    if (exclude != null && Math.abs(t - exclude) < 1e-9) continue;
    const px = Math.abs(t - position) * pxPerSecond;
    if (px <= tolerancePx && (best == null || Math.abs(t - position) < Math.abs(best - position))) {
      best = t;
    }
  }
  return best == null ? { position, snapped: false } : { position: best, snapped: true };
}

/** The timed item before `index` on a track, skipping a transition on the cut. */
export function leftOfCut(track: TimelineTrack, index: number): number | null {
  let i = index - 1;
  if (i >= 0 && track.items.find((x) => x.index === i)?.kind === "transition") i -= 1;
  return i >= 0 ? i : null;
}

export type Gesture =
  | { kind: "edge"; edge: "head" | "tail"; dxSeconds: number }
  | { kind: "body"; dxSeconds: number }
  | { kind: "click"; atSeconds: number };

/**
 * The edit a gesture asks for, or `null` when it asks for nothing (a zero-frame
 * drag, a razor on a gap, a slip on a gap).
 */
export function editFor(
  tool: Tool,
  gesture: Gesture,
  item: TimelineItem,
  track: TimelineTrack,
  rate: Rate,
): EditOp | null {
  if (item.kind === "transition") return null;
  if (gesture.kind === "click") {
    if (tool !== "razor") return null;
    const f = frameAt(gesture.atSeconds, rate);
    // Splitting exactly on an existing cut is not a split.
    if (f <= frameAt(item.at.start.seconds, rate) || f >= framesIn(item.at.end.seconds, rate)) {
      return null;
    }
    return { op: "split_at", time: exact(f, rate) };
  }
  const frames = framesIn(gesture.dxSeconds, rate);
  if (frames === 0) return null;
  const delta = exact(frames, rate);
  if (gesture.kind === "edge") {
    if (tool === "roll") {
      if (gesture.edge === "tail") return { op: "roll", cut: item.index, delta };
      const left = leftOfCut(track, item.index);
      return left == null ? null : { op: "roll", cut: left, delta };
    }
    if (tool === "select" || tool === "ripple") {
      return { op: "ripple_trim", index: item.index, edge: gesture.edge, delta };
    }
    return null;
  }
  if (item.kind !== "clip") return null;
  // Dragging a clip's content to the right shows material from earlier in the
  // source, which is a negative slip in the engine's convention.
  if (tool === "slip") return { op: "slip", index: item.index, delta: exact(-frames, rate) };
  if (tool === "slide") return { op: "slide", index: item.index, delta };
  return null;
}

/** An item's span on the track, as an exact range for lift and extract. */
export function spanOf(item: TimelineItem): ExactRange {
  return {
    start_time: { ticks: item.at.start.ticks, rate_num: item.at.start.rate_num, rate_den: item.at.start.rate_den },
    duration: {
      ticks: item.at.duration.ticks,
      rate_num: item.at.duration.rate_num,
      rate_den: item.at.duration.rate_den,
    },
  };
}

/** A human reading of a frame delta: `+12f`, `−3f`. */
export function frameLabel(frames: number): string {
  if (frames === 0) return "0f";
  return `${frames > 0 ? "+" : "−"}${Math.abs(frames)}f`;
}

/** How a render colours its pictures: as decoded, or through the project's view. */
export type RenderLook = "as_decoded" | "project_view";

/**
 * The parameters of a `render.sequence` job.
 *
 * With both record marks set, the render is the marked frames — the out mark
 * is on a picture that belongs to the range, so the end is one past it. With
 * either mark missing it is the whole cut: a half-marked range is not a range,
 * and guessing its other end would render something nobody asked for.
 */
export function renderParams(
  timeline: string,
  look: RenderLook,
  captions: string | null,
  marks: FrameMarks,
): Record<string, unknown> {
  const params: Record<string, unknown> = { timeline, look };
  if (captions) params.captions = captions;
  if (marks.in != null && marks.out != null && marks.out >= marks.in) {
    params.start_frame = marks.in;
    params.end_frame = marks.out + 1;
  }
  return params;
}
