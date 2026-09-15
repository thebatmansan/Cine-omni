import { useEffect, useRef, useState } from "react";
import type { ExactTime, PictureAt, RpcClient, TimelineView } from "../rpc/client";

/**
 * The record monitor: the picture the sequence shows at the playhead.
 *
 * Which clip, which asset and which picture of it are composition, so the
 * runtime answers them (`picture_at`); this panel only asks and displays. The
 * picture itself is a real decode from `/frame`. When the cut has nothing there
 * it shows black and says so; when the clip's media cannot be shown it says
 * why rather than showing a stale frame.
 */
export function Record({
  client,
  timeline,
  playhead,
  revision,
  active,
  onActivate,
  onStep,
}: {
  client: RpcClient;
  timeline: TimelineView | null;
  playhead: ExactTime | null;
  revision: number;
  active: boolean;
  onActivate(): void;
  onStep(by: number): void;
}) {
  const [at, setAt] = useState<PictureAt | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [decodeMs, setDecodeMs] = useState<number | null>(null);
  const inflight = useRef<AbortController | null>(null);

  const key = playhead ? `${playhead.ticks}/${playhead.rate_num}/${playhead.rate_den}` : "";

  useEffect(() => {
    if (!timeline || !playhead) {
      setAt(null);
      return;
    }
    let live = true;
    client
      .pictureAt(timeline.id, playhead)
      .then((p) => live && setAt(p))
      .catch((e: Error) => live && setFailure(e.message));
    return () => {
      live = false;
    };
    // The revision is a dependency on purpose: an edit under the playhead
    // changes what is there without the playhead moving.
  }, [client, timeline?.id, key, revision]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!at || at.black || !at.asset || at.index == null) {
      setUrl((old) => {
        if (old) URL.revokeObjectURL(old);
        return null;
      });
      setFailure(null);
      return;
    }
    inflight.current?.abort();
    const control = new AbortController();
    inflight.current = control;
    fetch(`/frame?media=${at.asset}&index=${at.index}`, { signal: control.signal })
      .then(async (r) => {
        if (!r.ok) {
          setFailure(await r.text());
          setUrl(null);
          return;
        }
        const blob = await r.blob();
        setUrl((old) => {
          if (old) URL.revokeObjectURL(old);
          return URL.createObjectURL(blob);
        });
        const ms = r.headers.get("x-cine-decode-ms");
        setDecodeMs(ms ? Number(ms) : null);
        setFailure(null);
      })
      .catch((e: Error) => {
        if (e.name !== "AbortError") setFailure(e.message);
      });
  }, [at]);

  return (
    <div className={`panel viewer record${active ? " active" : ""}`} onMouseDown={onActivate}>
      <div className="panel-head">
        <span>Record</span>
        {timeline && <span className="count">{timeline.name}</span>}
        <span className="spacer" />
        {decodeMs !== null && url && <span className="count">decode {decodeMs.toFixed(0)} ms</span>}
      </div>
      <div className="viewer-stage">
        {!timeline && <div className="viewer-empty">No sequence.</div>}
        {timeline && at?.black && <div className="viewer-empty">Black — nothing on a visible picture track here.</div>}
        {at && !at.black && at.index == null && (
          <div className="viewer-empty">
            “{at.name}” plays media that declares no available range, so which picture it shows cannot be known.
          </div>
        )}
        {at && !at.black && at.online === false && <div className="viewer-empty">“{at.name}”: media offline.</div>}
        {failure && <div className="viewer-empty">{failure}</div>}
        {url && !failure && <img src={url} alt="record picture" />}
        {at && !at.black && (
          <div className="viewer-note">
            {at.track_name} · {at.name}
            {at.source ? ` · src ${at.source.timecode}` : ""}
            {at.index != null ? ` · #${at.index}` : ""}
          </div>
        )}
      </div>
      <div className="transport">
        <button onClick={() => onStep(-1)} disabled={!timeline} title="Previous frame (←)">
          ◀
        </button>
        <button onClick={() => onStep(1)} disabled={!timeline} title="Next frame (→)">
          ▶
        </button>
        <span className="tc">{playhead ? timecodeOf(playhead) : "—"}</span>
        <span className="of">/ {timeline?.duration.timecode ?? "—"}</span>
        <span className="spacer" />
        <span className="of" style={{ color: "var(--text-faint)" }}>
          frame-accurate, not real-time
        </span>
      </div>
    </div>
  );
}

/** HH:MM:SS:FF for a whole-frame time at an integer-ish rate. Display only. */
export function timecodeOf(t: ExactTime): string {
  const fps = Math.round(t.rate_num / t.rate_den);
  const frames = t.ticks;
  const ff = ((frames % fps) + fps) % fps;
  const total = Math.floor(frames / fps);
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(h)}:${p(m)}:${p(s)}:${p(ff)}`;
}
