import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  EditOp,
  MarkMismatch,
  TimelineItem,
  TimelineTrack,
  TimelineView,
  TrackStateKey,
} from "../rpc/client";
import {
  TOOLS,
  Tool,
  canTrim,
  editFor,
  frameLabel,
  framesIn,
  sequenceRate,
  snap,
  snapTargets,
} from "../editing/edit";
import { tickPositions } from "./geometry";

/**
 * The cut, as the runtime composed it, and the tools that change it.
 *
 * Every position on screen comes from `item.at`, which the runtime computed
 * from exact rational time. Nothing here adds durations together: doing that in
 * the browser would be a second implementation of composition.
 *
 * A drag shows a guide and the frame count it means while the hand is moving —
 * that is gesture feedback, not project state. On release the gesture becomes
 * one edit, the runtime applies it (clamped to real media, carried to linked
 * material) and the redraw that follows is the runtime's answer.
 *
 * The record marks live here too. They are *marks*, not an edit: they say
 * where the next three-point edit lands and how long it is, and the band in
 * the ruler is the only thing drawn from them. What the marks mean — which
 * point is derived, whether the edit is backtimed, what happens when all four
 * disagree — is the runtime's arithmetic, not the browser's.
 */
export function Timeline({
  timeline,
  selected,
  onSelect,
  playheadSeconds,
  onScrub,
  onTrackState,
  onEdit,
  tool,
  onTool,
  snapping,
  onSnapping,
  linked,
  onLinked,
  lastApplied,
  marks,
  onMark,
  mismatch,
  onMismatch,
}: {
  timeline: TimelineView | null;
  selected: string | null;
  onSelect(item: TimelineItem, track: TimelineTrack): void;
  playheadSeconds: number;
  onScrub(seconds: number): void;
  onTrackState(track: string, key: TrackStateKey, value: boolean): void;
  onEdit(track: TimelineTrack, op: EditOp): void;
  tool: Tool;
  onTool(t: Tool): void;
  snapping: boolean;
  onSnapping(on: boolean): void;
  linked: boolean;
  onLinked(on: boolean): void;
  lastApplied: string | null;
  /** Record in/out in seconds, for the band; `null` is unmarked. */
  marks: { in: number | null; out: number | null };
  onMark(which: "in" | "out" | "clear"): void;
  mismatch: MarkMismatch;
  onMismatch(next: MarkMismatch): void;
}) {
  const [pxPerSecond, setPxPerSecond] = useState(90);
  const [drag, setDrag] = useState<Drag | null>(null);
  const lanes = useRef<HTMLDivElement>(null);

  const duration = timeline?.duration.seconds ?? 0;
  const width = Math.max(600, duration * pxPerSecond + 200);

  // Ctrl/⌘ + wheel zooms, as it does in every timeline a cutter has used.
  useEffect(() => {
    const el = lanes.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      setPxPerSecond((p) => Math.min(4000, Math.max(4, p * (1 - e.deltaY * 0.0015))));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const scrubTo = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const box = e.currentTarget.getBoundingClientRect();
      onScrub(Math.max(0, (e.clientX - box.left) / pxPerSecond));
    },
    [onScrub, pxPerSecond],
  );

  const ticks = useMemo(() => tickPositions(duration, pxPerSecond), [duration, pxPerSecond]);
  const targets = useMemo(
    () => (timeline ? snapTargets(timeline, playheadSeconds) : []),
    [timeline, playheadSeconds],
  );

  // The drag lives on the window so it survives leaving the clip.
  useEffect(() => {
    if (!drag || !timeline) return;
    const rate = sequenceRate(timeline);
    const move = (e: MouseEvent) => {
      const raw = (e.clientX - drag.startX) / pxPerSecond;
      const origin = drag.origin;
      let dx = raw;
      if (snapping && drag.gesture !== "slip") {
        const s = snap(origin + raw, targets, pxPerSecond, 8, origin);
        dx = s.position - origin;
        setDrag((d) => d && { ...d, dx, snapped: s.snapped });
        return;
      }
      setDrag((d) => d && { ...d, dx, snapped: false });
    };
    const up = () => {
      const gesture =
        drag.gesture === "head" || drag.gesture === "tail"
          ? { kind: "edge" as const, edge: drag.gesture, dxSeconds: drag.dx }
          : { kind: "body" as const, dxSeconds: drag.dx };
      const op = editFor(tool, gesture, drag.item, drag.track, rate);
      setDrag(null);
      if (op) onEdit(drag.track, op);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [drag, timeline, pxPerSecond, snapping, targets, tool, onEdit]);

  if (!timeline) {
    return (
      <div className="panel timeline">
        <div className="panel-head">Timeline</div>
        <div className="empty">
          No sequence in this project. Create one from <strong>Project → New sequence</strong>,
          or import a cut.
        </div>
      </div>
    );
  }

  const rate = sequenceRate(timeline);
  const dragFrames = drag ? framesIn(drag.dx, rate) : 0;

  return (
    <div className="panel timeline">
      <div className="panel-head timeline-head">
        <span>Timeline</span>
        <span className="count">{timeline.name}</span>
        <div className="toolbar" role="toolbar" aria-label="editing tools">
          {TOOLS.map((t) => (
            <button
              key={t.id}
              aria-pressed={tool === t.id}
              title={`${t.help} (${t.key.toUpperCase()})`}
              onClick={() => onTool(t.id)}
            >
              {t.label}
            </button>
          ))}
          <span className="sep" />
          <button aria-pressed={snapping} title="Snap to cuts and the playhead (S)" onClick={() => onSnapping(!snapping)}>
            Snap
          </button>
          <button
            aria-pressed={linked}
            title="Edits carry to linked picture and sound (⇧L)"
            onClick={() => onLinked(!linked)}
          >
            Linked
          </button>
          <span className="sep" />
          <button
            aria-pressed={marks.in != null}
            title="Mark the record in at the playhead (I)"
            onClick={() => onMark("in")}
          >
            Mark in
          </button>
          <button
            aria-pressed={marks.out != null}
            title="Mark the record out at the playhead (O): the edit is backtimed to end there"
            onClick={() => onMark("out")}
          >
            Mark out
          </button>
          <button
            title="Clear the record marks (⇧I)"
            disabled={marks.in == null && marks.out == null}
            onClick={() => onMark("clear")}
          >
            Clear
          </button>
          <label className="mismatch" title="What to do when all four marks are set and disagree">
            4-pt
            <select value={mismatch} onChange={(e) => onMismatch(e.target.value as MarkMismatch)}>
              <option value="refuse">refuse</option>
              <option value="trim_tail">trim source tail</option>
              <option value="trim_head">trim source head</option>
              <option value="honour_source">keep source length</option>
            <option value="fit_to_fill">fit to fill (change speed)</option>
            </select>
          </label>
        </div>
        <span className="spacer" />
        {drag && (
          <span className={`count drag-readout${drag.snapped ? " snapped" : ""}`}>
            {frameLabel(dragFrames)}
            {drag.snapped ? " · snapped" : ""}
          </span>
        )}
        {!drag && lastApplied && <span className="count">{lastApplied}</span>}
        <span className="count">{timeline.duration.timecode}</span>
        <span className="count">{Math.round(pxPerSecond)} px/s</span>
      </div>
      <div className={`timeline-body tool-${tool}`} ref={lanes}>
        <div className="timeline-grid" style={{ gridTemplateColumns: `150px ${width}px` }}>
          <div className="ruler-corner" />
          <div className="ruler">
            <div className="ruler-canvas" style={{ width }} onMouseDown={scrubTo}>
              {ticks.map((t) => (
                <div key={t.seconds}>
                  <div className="ruler-tick" style={{ left: t.seconds * pxPerSecond }} />
                  <div className="ruler-label" style={{ left: t.seconds * pxPerSecond }}>
                    {t.label}
                  </div>
                </div>
              ))}
              {markBand(marks, duration, pxPerSecond)}
              <div className="playhead" style={{ left: playheadSeconds * pxPerSecond }} />
            </div>
          </div>

          {timeline.tracks.map((track) => (
            <TrackRow
              key={track.id ?? track.index}
              track={track}
              width={width}
              pxPerSecond={pxPerSecond}
              playheadSeconds={playheadSeconds}
              selected={selected}
              tool={tool}
              drag={drag}
              onSelect={onSelect}
              onScrub={scrubTo}
              onTrackState={onTrackState}
              onBeginDrag={(item, gesture, e) => {
                const origin =
                  gesture === "tail" ? item.at.end.seconds : item.at.start.seconds;
                setDrag({ item, track, gesture, startX: e.clientX, origin, dx: 0, snapped: false });
              }}
              onRazor={(item, e) => {
                const box = (e.currentTarget.parentElement as HTMLElement).getBoundingClientRect();
                let at = (e.clientX - box.left) / pxPerSecond;
                if (snapping) at = snap(at, targets, pxPerSecond).position;
                const op = editFor("razor", { kind: "click", atSeconds: at }, item, track, rate);
                if (op) onEdit(track, op);
              }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

/**
 * The band between the record marks.
 *
 * One mark alone still means something: an in with no out runs to the end of
 * the sequence, an out with no in runs back to its start, because that is the
 * span the edit will be measured against once the other point arrives.
 */
function markBand(
  marks: { in: number | null; out: number | null },
  duration: number,
  pxPerSecond: number,
) {
  if (marks.in == null && marks.out == null) return null;
  const from = marks.in ?? 0;
  const to = marks.out ?? duration;
  return (
    <>
      <div
        className="mark-band"
        aria-label="record marks"
        style={{ left: from * pxPerSecond, width: Math.max(1, (to - from) * pxPerSecond) }}
      />
      {marks.in != null && <div className="mark mark-in" style={{ left: marks.in * pxPerSecond }} />}
      {marks.out != null && <div className="mark mark-out" style={{ left: marks.out * pxPerSecond }} />}
    </>
  );
}

interface Drag {
  item: TimelineItem;
  track: TimelineTrack;
  gesture: "head" | "tail" | "slip" | "slide";
  startX: number;
  /** Where the dragged edge (or the clip's head) started, in seconds. */
  origin: number;
  dx: number;
  snapped: boolean;
}

function TrackRow({
  track,
  width,
  pxPerSecond,
  playheadSeconds,
  selected,
  tool,
  drag,
  onSelect,
  onScrub,
  onTrackState,
  onBeginDrag,
  onRazor,
}: {
  track: TimelineTrack;
  width: number;
  pxPerSecond: number;
  playheadSeconds: number;
  selected: string | null;
  tool: Tool;
  drag: Drag | null;
  onSelect(item: TimelineItem, track: TimelineTrack): void;
  onScrub(e: React.MouseEvent<HTMLDivElement>): void;
  onTrackState(track: string, key: TrackStateKey, value: boolean): void;
  onBeginDrag(item: TimelineItem, gesture: Drag["gesture"], e: React.MouseEvent): void;
  onRazor(item: TimelineItem, e: React.MouseEvent<HTMLDivElement>): void;
}) {
  const flag = (key: TrackStateKey, label: string, on: boolean, title: string) => (
    <button
      className={key === "solo" ? "solo" : undefined}
      aria-pressed={on}
      aria-label={`${track.name} ${key}`}
      title={title}
      disabled={!track.id}
      onClick={() => track.id && onTrackState(track.id, key, !on)}
    >
      {label}
    </button>
  );

  const guide =
    drag && drag.track.id === track.id
      ? (drag.gesture === "tail" || drag.gesture === "head" || drag.gesture === "slide"
          ? drag.origin + drag.dx
          : null)
      : null;

  return (
    <>
      <div className={`track-head${track.enabled ? "" : " disabled"}`}>
        <div className="name">
          <span className="kind">{track.kind === "video" ? "V" : "A"}</span>
          <span>{track.name}</span>
        </div>
        <div className="flags">
          {flag("locked", "L", track.locked, "Lock: refuses every edit to this track")}
          {flag("muted", "M", track.muted, "Mute")}
          {flag("solo", "S", track.solo, "Solo")}
          {flag("targeted", "T", track.targeted, "Target for insert and overwrite")}
        </div>
      </div>
      <div
        className={`track-lane${track.locked ? " locked" : ""}`}
        style={{ width }}
        onMouseDown={onScrub}
      >
        {track.items.map((item) => (
          <Clip
            key={item.id ?? `${item.index}-${item.name}`}
            item={item}
            track={track}
            pxPerSecond={pxPerSecond}
            selected={selected === itemKey(item, track)}
            tool={tool}
            dragging={drag?.item.id === item.id && drag?.track.id === track.id}
            onSelect={onSelect}
            onBeginDrag={onBeginDrag}
            onRazor={onRazor}
          />
        ))}
        {guide != null && <div className="edit-guide" style={{ left: guide * pxPerSecond }} />}
        <div className="playhead" style={{ left: playheadSeconds * pxPerSecond }} />
      </div>
    </>
  );
}

function Clip({
  item,
  track,
  pxPerSecond,
  selected,
  tool,
  dragging,
  onSelect,
  onBeginDrag,
  onRazor,
}: {
  item: TimelineItem;
  track: TimelineTrack;
  pxPerSecond: number;
  selected: boolean;
  tool: Tool;
  dragging: boolean;
  onSelect(item: TimelineItem, track: TimelineTrack): void;
  onBeginDrag(item: TimelineItem, gesture: Drag["gesture"], e: React.MouseEvent): void;
  onRazor(item: TimelineItem, e: React.MouseEvent<HTMLDivElement>): void;
}) {
  const left = item.at.start.seconds * pxPerSecond;
  const width = Math.max(2, item.at.duration.seconds * pxPerSecond);
  const kindClass =
    item.kind === "clip" ? track.kind : item.kind === "gap" ? "gap" : "transition";
  const editable = item.kind !== "transition" && !track.locked;
  const edges = canTrim(item, track, tool) && width > 10;
  const title = [
    `${item.name} · ${item.at.start.timecode} → ${item.at.end.timecode}`,
    item.needs_conform ? `Needs conform: ${item.conform_reason ?? "source changed"}` : null,
    item.link ? `Linked: ${item.link}` : null,
    item.scenes.length ? `Bound to ${item.scenes.length} scene(s)` : null,
    item.speed
      ? `Retimed: ${item.speed.media}/${item.speed.track}${item.speed.reversed ? " reversed" : ""}`
      : null,
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <div
      className={`clip ${kindClass}${item.enabled ? "" : " disabled"}${item.needs_conform ? " conform" : ""}${dragging ? " dragging" : ""}`}
      style={{ left, width }}
      aria-selected={selected}
      title={title}
      onMouseDown={(e) => {
        e.stopPropagation();
        onSelect(item, track);
        if (!editable) return;
        if (tool === "razor") onRazor(item, e);
        else if (tool === "slip" || tool === "slide") onBeginDrag(item, tool, e);
      }}
    >
      {edges && (
        <div
          className="clip-edge head"
          onMouseDown={(e) => {
            e.stopPropagation();
            onSelect(item, track);
            onBeginDrag(item, "head", e);
          }}
        />
      )}
      {width > 26 && (
        <div className="clip-name">
          {item.needs_conform && <span className="glyph conform-glyph">!</span>}
          {item.link && <span className="glyph">∞</span>}
          {item.scenes.length > 0 && <span className="glyph">SC</span>}
          {item.speed && <span className="glyph speed-glyph">{item.speed.label}</span>}
          {item.name}
        </div>
      )}
      {edges && (
        <div
          className="clip-edge tail"
          onMouseDown={(e) => {
            e.stopPropagation();
            onSelect(item, track);
            onBeginDrag(item, "tail", e);
          }}
        />
      )}
    </div>
  );
}

export function itemKey(item: TimelineItem, track: TimelineTrack): string {
  return item.id ?? `${track.index}:${item.index}`;
}
