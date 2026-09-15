import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { TimelineItem, TimelineTrack, TimelineView, TrackStateKey } from "../rpc/client";
import { tickPositions } from "./geometry";

/**
 * The cut, as the runtime composed it.
 *
 * Every position on screen comes from `item.at`, which the runtime computed
 * from exact rational time. Nothing here adds durations together: doing that in
 * the browser would be a second implementation of composition, in a language
 * with one number type, and it would eventually disagree with the engine.
 *
 * `seconds` is used only to turn a time into an x coordinate, which is the one
 * place a float cannot cause a wrong cut.
 */
export function Timeline({
  timeline,
  selected,
  onSelect,
  playhead,
  onScrub,
  onTrackState,
}: {
  timeline: TimelineView | null;
  selected: string | null;
  onSelect(item: TimelineItem, track: TimelineTrack): void;
  playhead: number;
  onScrub(seconds: number): void;
  onTrackState(track: string, key: TrackStateKey, value: boolean): void;
}) {
  const [pxPerSecond, setPxPerSecond] = useState(90);
  const lanes = useRef<HTMLDivElement>(null);

  const duration = timeline?.duration.seconds ?? 0;
  const width = Math.max(600, duration * pxPerSecond + 40);

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

  if (!timeline) {
    return (
      <div className="panel timeline">
        <div className="panel-head">Timeline</div>
        <div className="empty">
          No sequence in this project.
          <br />
          Import one with <code>cine import</code>, or open a project that has a cut.
        </div>
      </div>
    );
  }

  return (
    <div className="panel timeline">
      <div className="panel-head">
        <span>Timeline</span>
        <span className="count">{timeline.name}</span>
        <span className="count">{timeline.duration.timecode}</span>
        <span className="spacer" />
        <span className="count">{Math.round(pxPerSecond)} px/s · ⌘-scroll to zoom</span>
      </div>
      <div className="timeline-body" ref={lanes}>
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
              <div className="playhead" style={{ left: playhead * pxPerSecond }} />
            </div>
          </div>

          {timeline.tracks.map((track) => (
            <TrackRow
              key={track.index}
              track={track}
              width={width}
              pxPerSecond={pxPerSecond}
              playhead={playhead}
              selected={selected}
              onSelect={onSelect}
              onScrub={scrubTo}
              onTrackState={onTrackState}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

function TrackRow({
  track,
  width,
  pxPerSecond,
  playhead,
  selected,
  onSelect,
  onScrub,
  onTrackState,
}: {
  track: TimelineTrack;
  width: number;
  pxPerSecond: number;
  playhead: number;
  selected: string | null;
  onSelect(item: TimelineItem, track: TimelineTrack): void;
  onScrub(e: React.MouseEvent<HTMLDivElement>): void;
  onTrackState(track: string, key: TrackStateKey, value: boolean): void;
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
            key={`${item.index}-${item.name}`}
            item={item}
            track={track}
            pxPerSecond={pxPerSecond}
            selected={selected === itemKey(item, track)}
            onSelect={onSelect}
          />
        ))}
        <div className="playhead" style={{ left: playhead * pxPerSecond }} />
      </div>
    </>
  );
}

function Clip({
  item,
  track,
  pxPerSecond,
  selected,
  onSelect,
}: {
  item: TimelineItem;
  track: TimelineTrack;
  pxPerSecond: number;
  selected: boolean;
  onSelect(item: TimelineItem, track: TimelineTrack): void;
}) {
  const left = item.at.start.seconds * pxPerSecond;
  const width = Math.max(2, item.at.duration.seconds * pxPerSecond);
  const kindClass =
    item.kind === "clip" ? track.kind : item.kind === "gap" ? "gap" : "transition";
  return (
    <div
      className={`clip ${kindClass}${item.enabled ? "" : " disabled"}`}
      style={{ left, width }}
      aria-selected={selected}
      title={`${item.name} · ${item.at.start.timecode} → ${item.at.end.timecode}`}
      onMouseDown={(e) => {
        e.stopPropagation();
        onSelect(item, track);
      }}
    >
      {width > 26 && <div className="clip-name">{item.name}</div>}
    </div>
  );
}

export function itemKey(item: TimelineItem, track: TimelineTrack): string {
  return item.id ?? `${track.index}:${item.index}`;
}
