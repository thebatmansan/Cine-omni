import { useEffect, useRef, useState } from "react";
import type {
  AngleAt,
  ExactTime,
  MediaPreference,
  PictureAt,
  RpcClient,
  TimelineView,
} from "../rpc/client";

/**
 * The record monitor: the picture the sequence shows at the playhead.
 *
 * Which clip, which asset and which picture of it are composition, so the
 * runtime answers them (`picture_at`); this panel only asks and displays. The
 * picture itself is a real decode from `/frame`. When the cut has nothing there
 * it shows black and says so; when the clip's media cannot be shown it says
 * why rather than showing a stale frame.
 *
 * This is the record monitor, so the picture is the clip's: asked for through
 * the project's view with that clip's grade applied, which is what the cut
 * looks like. The runtime says whether that is available (`colour.available`)
 * and this asks for it only then — a media with no input colorspace has no
 * view to look through, and the raw decode is shown with the reason said. The
 * clip is in the request, so two clips that share one file cannot be served
 * each other's grade, and the revision is in it too, so no cache anywhere can
 * answer a regrade with the picture from before it.
 */
export function Record({
  client,
  timeline,
  playhead,
  revision,
  preference,
  active,
  onActivate,
  onStep,
  onSwitchAngle,
}: {
  client: RpcClient;
  timeline: TimelineView | null;
  playhead: ExactTime | null;
  revision: number;
  /** What playback reads. Changing it changes which file this shows, so it is
   *  a dependency of the question, not decoration. */
  preference: MediaPreference;
  active: boolean;
  onActivate(): void;
  onStep(by: number): void;
  /** Cut to an angle at the playhead. */
  onSwitchAngle?(key: string): void;
}) {
  const [multicam, setMulticam] = useState(false);
  const [at, setAt] = useState<PictureAt | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [decodeMs, setDecodeMs] = useState<number | null>(null);
  /** The clip whose grade the runtime says this picture was served through. */
  const [coloured, setColoured] = useState<string | null>(null);
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
    // changes what is there without the playhead moving. So is the media
    // preference: the same instant of the same cut may be read from another
    // file.
  }, [client, timeline?.id, key, revision, preference]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!at || at.black || !at.asset || at.index == null) {
      setUrl((old) => {
        if (old) URL.revokeObjectURL(old);
        return null;
      });
      setFailure(null);
      setColoured(null);
      return;
    }
    inflight.current?.abort();
    const control = new AbortController();
    inflight.current = control;
    const graded = at.colour?.available && at.clip ? `&view=project&clip=${at.clip}` : "";
    fetch(
      `/frame?media=${at.asset}&index=${at.index}${at.proxy ? `&proxy=${at.proxy}` : ""}${graded}&rev=${revision}`,
      { signal: control.signal },
    )
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
        // The clip the runtime says it coloured for, not the one this asked
        // for: if they ever differed, the monitor would be showing another
        // clip's grade and calling it this one's.
        setColoured(r.headers.get("x-cine-clip"));
        setFailure(null);
      })
      .catch((e: Error) => {
        if (e.name !== "AbortError") setFailure(e.message);
      });
  }, [at, revision]);

  return (
    <div className={`panel viewer record${active ? " active" : ""}`} onMouseDown={onActivate}>
      <div className="panel-head">
        <span>Record</span>
        {timeline && <span className="count">{timeline.name}</span>}
        <span className="spacer" />
        {decodeMs !== null && url && !multicam && <span className="count">decode {decodeMs.toFixed(0)} ms</span>}
        {onSwitchAngle && (
          <button
            aria-pressed={multicam}
            title="Show every angle of the multicam clip under the playhead; click one to cut to it"
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => setMulticam((m) => !m)}
          >
            Angles
          </button>
        )}
      </div>
      {multicam && timeline && playhead && onSwitchAngle ? (
        <AngleGrid
          client={client}
          timeline={timeline.id}
          playhead={playhead}
          revision={revision}
          onSwitch={onSwitchAngle}
        />
      ) : (
      <div className="viewer-stage">
        {!timeline && <div className="viewer-empty">No sequence.</div>}
        {timeline && at?.black && <div className="viewer-empty">Black — nothing on a visible picture track here.</div>}
        {at && !at.black && at.index == null && (
          <div className="viewer-empty">
            “{at.name}” plays media that declares no available range, so which picture it shows cannot be known.
          </div>
        )}
        {/* The edit list carries the same availability the media list shows,
            so the monitor cannot contradict the browser about one take. */}
        {at && !at.black && at.online === false && (
          <div className="viewer-empty">“{at.name}”: media offline.</div>
        )}
        {failure && <div className="viewer-empty">{failure}</div>}
        {url && !failure && <img src={url} alt="record picture" />}
        {at && !at.black && (
          <div className="viewer-note">
            {at.proxy && <span className="chip proxy" title={at.substitution ?? "reading a proxy"}>PROXY</span>}
            {/* What colour this picture is, which for a record monitor is not
                decoration: a graded picture and an ungraded one are different
                answers to "what does the cut look like here". */}
            {coloured && coloured === at.clip ? (
              <span className="chip" title="through the project's view, with this clip's grade">
                {at.colour?.graded ? "GRADED" : "VIEW"}
              </span>
            ) : (
              at.colour?.why_not && (
                <span className="chip dim" title={at.colour.why_not}>
                  RAW
                </span>
              )
            )}
            {at.track_name} · {at.name}
            {at.source ? ` · src ${at.source.timecode}` : ""}
            {at.index != null ? ` · #${at.index}` : ""}
            {at.substitution ? ` · ${at.substitution}` : ""}
          </div>
        )}
      </div>
      )}
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

/**
 * Every angle of the multicam clip under the playhead, decoded.
 *
 * Which picture of which file each angle shows is the runtime's answer
 * (`angles_at`): the take's moment carried into each camera's own media through
 * the offsets a sync recorded. Each cell is a real decode from `/frame`. An
 * angle that was not rolling at that moment says so instead of showing black.
 * Clicking a cell cuts to that angle at the playhead. Like the record monitor
 * this steps; it does not play.
 */
function AngleGrid({
  client,
  timeline,
  playhead,
  revision,
  onSwitch,
}: {
  client: RpcClient;
  timeline: string;
  playhead: ExactTime;
  revision: number;
  onSwitch(key: string): void;
}) {
  const [angles, setAngles] = useState<AngleAt[] | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const key = `${playhead.ticks}/${playhead.rate_num}/${playhead.rate_den}`;
  useEffect(() => {
    let live = true;
    client
      .anglesAt(timeline, playhead)
      .then((v) => {
        if (!live) return;
        setAngles(v.clip ? v.angles : []);
        setFailure(null);
      })
      .catch((e: Error) => live && setFailure(e.message));
    return () => {
      live = false;
    };
  }, [client, timeline, key, revision]); // eslint-disable-line react-hooks/exhaustive-deps

  if (failure) return <div className="viewer-stage"><div className="viewer-empty">{failure}</div></div>;
  if (!angles) return <div className="viewer-stage" />;
  if (angles.length === 0) {
    return (
      <div className="viewer-stage">
        <div className="viewer-empty">The clip under the playhead has one angle.</div>
      </div>
    );
  }
  return (
    <div className={`angle-grid n${Math.min(angles.length, 4)}`}>
      {angles.slice(0, 4).map((a) => (
        <AngleCell key={a.key} angle={a} onSwitch={() => onSwitch(a.key)} />
      ))}
      {angles.length > 4 && <div className="viewer-note">{angles.length - 4} more angles not shown</div>}
    </div>
  );
}

function AngleCell({ angle, onSwitch }: { angle: AngleAt; onSwitch(): void }) {
  const [url, setUrl] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    if (!angle.asset || angle.index == null) {
      setUrl(null);
      return;
    }
    const control = new AbortController();
    fetch(`/frame?media=${angle.asset}&index=${angle.index}`, { signal: control.signal })
      .then(async (r) => {
        if (!r.ok) {
          setFailure(await r.text());
          return;
        }
        const blob = await r.blob();
        setUrl((old) => {
          if (old) URL.revokeObjectURL(old);
          return URL.createObjectURL(blob);
        });
        setFailure(null);
      })
      .catch((e: Error) => e.name !== "AbortError" && setFailure(e.message));
    return () => control.abort();
  }, [angle.asset, angle.index]);
  return (
    <button
      className={`angle-cell${angle.active ? " live" : ""}`}
      title={angle.active ? `${angle.name} is live` : `Cut to ${angle.name} at the playhead`}
      disabled={angle.active || angle.index == null}
      onClick={onSwitch}
    >
      {url && angle.index != null && !failure ? (
        <img src={url} alt={angle.name} />
      ) : (
        <span className="viewer-empty">{angle.why_not ?? failure ?? "…"}</span>
      )}
      <span className="angle-label">
        {angle.key === "DEFAULT_MEDIA" ? "A" : angle.key} · {angle.name}
        {angle.index != null ? ` · #${angle.index}` : ""}
      </span>
    </button>
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
