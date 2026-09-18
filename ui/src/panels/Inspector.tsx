import { Fragment } from "react";
import { useState } from "react";
import { ExactTime, MediaView, ScriptView, TimelineItem, TimelineTrack, TimelineView } from "../rpc/client";

/** Whatever is selected, in detail. Values come from the projection unchanged. */
export function Inspector({
  timeline,
  selection,
  media,
  script,
  onRelink,
  onConformDone,
  onBind,
  onUnbind,
  onJob,
  onRetime,
  onAngle,
}: {
  timeline: TimelineView | null;
  selection: { item: TimelineItem; track: TimelineTrack } | null;
  media: MediaView | null;
  script?: ScriptView | null;
  onConformDone?(clip: string): void;
  onBind?(clip: string, scene: string): void;
  onUnbind?(clip: string, scene: string): void;
  onJob?(kind: string, params: Record<string, unknown>): void;
  /** Relink one asset to one file the person names. */
  onRelink?(media: string, path: string): void;
  onRetime?(
    clip: string,
    played: ExactTime | null,
    reversed: boolean,
    keepPitch: boolean,
    ramp?: RampRow[] | null,
  ): void;
  onAngle?(clip: string, key: string): void;
}) {
  return (
    <div className="inspector">
      <div className="panel-body pad">
        {selection && (
          <>
            <ClipDetail item={selection.item} track={selection.track} />
            {selection.item.kind === "clip" && selection.item.id && onAngle && (
              <Angles
                item={selection.item}
                clip={selection.item.id}
                onAngle={onAngle}
                onSync={onJob ? (clip) => onJob("multicam.sync", { clip }) : undefined}
              />
            )}
            {selection.item.kind === "clip" && selection.item.id && onRetime && (
              <Speed item={selection.item} clip={selection.item.id} onRetime={onRetime} />
            )}
            {selection.item.kind === "clip" && selection.item.id && (
              <Context
                item={selection.item}
                clip={selection.item.id}
                script={script ?? null}
                onConformDone={onConformDone}
                onBind={onBind}
                onUnbind={onUnbind}
              />
            )}
          </>
        )}
        {!selection && media && <MediaDetail media={media} onJob={onJob} onRelink={onRelink} />}
        {!selection && !media && (
          <div className="empty">Select a clip or a media item.</div>
        )}
        {timeline && !selection && !media && (
          <>
            <div className="section-title">Sequence</div>
            <dl className="props">
              <dt>name</dt>
              <dd>{timeline.name}</dd>
              <dt>duration</dt>
              <dd>{timeline.duration.timecode}</dd>
              <dt>tracks</dt>
              <dd>{timeline.tracks.length}</dd>
            </dl>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * The angles a multicam clip could play.
 *
 * Choosing one here changes this clip only — it does not cut. Cutting between
 * angles is `1`..`9` at the playhead, which splits and plays the new angle from
 * that point, because that is what a multicam switch is.
 */
function Angles({
  item,
  clip,
  onAngle,
  onSync,
}: {
  item: TimelineItem;
  clip: string;
  onAngle(clip: string, key: string): void;
  onSync?(clip: string): void;
}) {
  if (!item.angles || item.angles.length < 2) return null;
  return (
    <>
      <div className="section-title">Angles</div>
      <div className="chips" role="group" aria-label="angles">
        {item.angles.map((key) => (
          <button
            key={key}
            className="chip"
            aria-pressed={key === item.angle}
            onClick={() => onAngle(clip, key)}
          >
            {key}
          </button>
        ))}
      </div>
      {onSync && (
        <div className="row">
          <button
            title="Line the angles up by what their microphones heard. Runs in the background; an angle whose sound does not clearly match is left alone and reported."
            onClick={() => onSync(clip)}
          >
            Sync by sound
          </button>
        </div>
      )}
      <div className="note">
        An angle the runtime refuses is one whose camera was not rolling over this
        part of the take; it says how much is missing rather than cutting to black.
      </div>
    </>
  );
}

/** One ramp segment as sent to the runtime: speeds exactly as typed. */
export interface RampRow {
  length: ExactTime;
  from: string;
  to: string;
}

/**
 * A speed ramp, segment by segment.
 *
 * Each row is a length in frames and the speed at its start and end, typed as
 * an editor types it — `100%`, `3/2`, `1.5`. The browser does no arithmetic on
 * them: the runtime parses each exactly, works out the media the ramp consumes
 * and the length it takes, and refuses a speed it cannot read or a ramp that
 * runs past the shot, saying which.
 */
function RampEditor({ item, onApply }: { item: TimelineItem; onApply(rows: RampRow[]): void }) {
  const rate = { num: item.at.duration.rate_num, den: item.at.duration.rate_den };
  const ratio = ([n, d]: [number, number]) => (d === 1 ? `${n}` : `${n}/${d}`);
  const fromItem = () =>
    item.ramp?.map((s) => ({ frames: String(s.length.ticks), from: ratio(s.from), to: ratio(s.to) })) ?? [
      { frames: String(item.at.duration.ticks), from: "100%", to: "200%" },
    ];
  const [rows, setRows] = useState(fromItem);
  const [open, setOpen] = useState(item.ramp != null);
  const set = (i: number, key: "frames" | "from" | "to", v: string) =>
    setRows((r) => r.map((row, j) => (j === i ? { ...row, [key]: v } : row)));
  const valid = rows.length > 0 && rows.every((r) => Number.isInteger(Number(r.frames)) && Number(r.frames) > 0);

  if (!open) {
    return (
      <div className="row">
        <button onClick={() => setOpen(true)}>Speed ramp…</button>
      </div>
    );
  }
  return (
    <div className="ramp">
      <div className="ramp-row head">
        <span>frames</span>
        <span>from</span>
        <span>to</span>
        <span />
      </div>
      {rows.map((r, i) => (
        <div className="ramp-row" key={i}>
          <input aria-label={`segment ${i + 1} frames`} value={r.frames} onChange={(e) => set(i, "frames", e.target.value)} />
          <input aria-label={`segment ${i + 1} start speed`} value={r.from} onChange={(e) => set(i, "from", e.target.value)} />
          <input aria-label={`segment ${i + 1} end speed`} value={r.to} onChange={(e) => set(i, "to", e.target.value)} />
          <button aria-label={`remove segment ${i + 1}`} disabled={rows.length === 1} onClick={() => setRows(rows.filter((_, j) => j !== i))}>
            ×
          </button>
        </div>
      ))}
      <div className="row">
        <button
          onClick={() => {
            const last = rows[rows.length - 1];
            setRows([...rows, { frames: "24", from: last?.to ?? "100%", to: last?.to ?? "100%" }]);
          }}
        >
          Add segment
        </button>
        <button
          disabled={!valid}
          onClick={() =>
            onApply(
              rows.map((r) => ({
                length: { ticks: Number(r.frames), rate_num: rate.num, rate_den: rate.den },
                from: r.from,
                to: r.to,
              })),
            )
          }
        >
          Apply ramp
        </button>
      </div>
    </div>
  );
}

/**
 * A clip's speed.
 *
 * The control asks for a *length*, not a percentage, because a length is what
 * a retime exactly is: this many frames of media, over that many of timeline.
 * A percentage would have to become a length anyway, and the rounding would
 * happen somewhere nobody could see it. The speed shown beside it is the
 * runtime's own ratio.
 */
function Speed({
  item,
  clip,
  onRetime,
}: {
  item: TimelineItem;
  clip: string;
  onRetime(
    clip: string,
    played: ExactTime | null,
    reversed: boolean,
    keepPitch: boolean,
    ramp?: RampRow[] | null,
  ): void;
}) {
  const [frames, setFrames] = useState("");
  if (!item.source_range) return null;
  const rate = { num: item.at.duration.rate_num, den: item.at.duration.rate_den };
  const current = item.at.duration.ticks;
  const media = item.source_range.duration;
  const wanted = Number(frames);

  return (
    <>
      <div className="section-title">Speed</div>
      <dl className="props">
        <dt>plays</dt>
        <dd>{item.speed ? item.speed.label : "real time"}</dd>
        <dt>media</dt>
        <dd>
          {media.ticks} frames @ {media.rate_num}/{media.rate_den}
        </dd>
        <dt>on the cut</dt>
        <dd>{current} frames</dd>
      </dl>
      <div className="row">
        <input
          type="number"
          min={1}
          placeholder={String(current)}
          value={frames}
          aria-label="length on the timeline in frames"
          onChange={(e) => setFrames(e.target.value)}
        />
        <button
          disabled={!Number.isInteger(wanted) || wanted < 1}
          onClick={() => {
            onRetime(
              clip,
              { ticks: wanted, rate_num: rate.num, rate_den: rate.den },
              item.speed?.reversed ?? false,
              item.keep_pitch,
            );
            setFrames("");
          }}
        >
          Fit to length
        </button>
        <button
          title="Play the media backwards over the same length"
          onClick={() => onRetime(clip, item.at.duration, !(item.speed?.reversed ?? false), item.keep_pitch)}
        >
          {item.speed?.reversed ? "Forwards" : "Backwards"}
        </button>
        <button disabled={!item.speed} onClick={() => onRetime(clip, null, false, false)}>
          Real time
        </button>
      </div>
      <RampEditor item={item} onApply={(rows) => onRetime(clip, null, item.speed?.reversed ?? false, item.keep_pitch, rows)} />
      {item.speed && (
        <label className="row" title="Keep the sound's pitch while its speed changes (a time stretch), rather than varispeed">
          <input
            type="checkbox"
            checked={item.keep_pitch}
            onChange={(e) => onRetime(clip, item.at.duration, item.speed?.reversed ?? false, e.target.checked)}
          />
          Keep pitch
        </label>
      )}
      {item.speed && (
        <div className="note">
          {item.keep_pitch
            ? "Sound keeps its pitch (a time stretch); sharp attacks can flam. "
            : "Sound plays as varispeed: its pitch follows the speed. "}
          Trims, rolls and joins refuse a retimed clip rather than guess whether you meant to keep
          the speed or the media. Set the length again instead.
        </div>
      )}
    </>
  );
}

/** Where a clip stands in the production: conform state and scene bindings. */
function Context({
  item,
  clip,
  script,
  onConformDone,
  onBind,
  onUnbind,
}: {
  item: TimelineItem;
  clip: string;
  script: ScriptView | null;
  onConformDone?(clip: string): void;
  onBind?(clip: string, scene: string): void;
  onUnbind?(clip: string, scene: string): void;
}) {
  const scenes = script?.documents.flatMap((d) => d.scenes) ?? [];
  const [pick, setPick] = useState("");
  const bound = scenes.filter((s) => item.scenes.includes(s.id));
  const unbound = scenes.filter((s) => !item.scenes.includes(s.id));
  return (
    <>
      <div className="section-title">Conform</div>
      {item.needs_conform ? (
        <div className="notice">
          <strong>Needs conforming</strong>
          {item.conform_reason ?? "Its source changed."}
          <div style={{ marginTop: 6 }}>
            <button onClick={() => onConformDone?.(clip)}>Mark conformed</button>
          </div>
        </div>
      ) : (
        <div className="hint">Up to date with everything it is bound to.</div>
      )}
      <div className="section-title">Scenes</div>
      {bound.length === 0 && <div className="hint">Not bound to any scene; script changes will not reach it.</div>}
      {bound.map((s) => (
        <div key={s.id} className="binding">
          <span className="label">{s.heading}</span>
          <button onClick={() => onUnbind?.(clip, s.id)}>Unbind</button>
        </div>
      ))}
      {unbound.length > 0 && (
        <div className="binding">
          <select value={pick} onChange={(e) => setPick(e.target.value)} aria-label="scene to bind">
            <option value="">Bind to scene…</option>
            {unbound.map((s) => (
              <option key={s.id} value={s.id}>
                {s.number ? `${s.number}. ` : ""}
                {s.heading}
              </option>
            ))}
          </select>
          <button
            disabled={!pick}
            onClick={() => {
              onBind?.(clip, pick);
              setPick("");
            }}
          >
            Bind
          </button>
        </div>
      )}
    </>
  );
}

function ClipDetail({ item, track }: { item: TimelineItem; track: TimelineTrack }) {
  return (
    <>
      <div className="section-title">{item.kind}</div>
      <dl className="props">
        <dt>name</dt>
        <dd>{item.name}</dd>
        <dt>track</dt>
        <dd>
          {track.name} ({track.kind})
        </dd>
        <dt>record in</dt>
        <dd>{item.at.start.timecode}</dd>
        <dt>record out</dt>
        <dd>{item.at.end.timecode}</dd>
        <dt>duration</dt>
        <dd>{item.at.duration.timecode}</dd>
        <dt>exact</dt>
        <dd>
          {item.at.duration.ticks} @ {item.at.duration.rate_num}/{item.at.duration.rate_den}
        </dd>
        <dt>enabled</dt>
        <dd>{String(item.enabled)}</dd>
      </dl>
      {item.source_range && (
        <>
          <div className="section-title">Source</div>
          <dl className="props">
            <dt>in</dt>
            <dd>{item.source_range.start.timecode}</dd>
            <dt>out</dt>
            <dd>{item.source_range.end.timecode}</dd>
          </dl>
        </>
      )}
      {item.handles && (
        <>
          <div className="section-title">Handles</div>
          <dl className="props">
            <dt>head</dt>
            <dd>{item.handles.head.timecode}</dd>
            <dt>tail</dt>
            <dd>{item.handles.tail.timecode}</dd>
          </dl>
        </>
      )}
      {!item.handles && item.kind === "clip" && (
        <div className="notice">
          <strong>Handles unknown</strong>
          This clip declares no available range, so how much media lies either side cannot be
          answered. That is not the same as no handles.
        </div>
      )}
      {item.media && (
        <>
          <div className="section-title">Media reference</div>
          <dl className="props">
            <dt>target</dt>
            <dd>{item.media}</dd>
          </dl>
        </>
      )}
      {item.link && (
        <>
          <div className="section-title">Linked</div>
          <dl className="props">
            <dt>group</dt>
            <dd>{item.link}</dd>
          </dl>
        </>
      )}
    </>
  );
}

function MediaDetail({
  media,
  onJob,
  onRelink,
}: {
  media: MediaView;
  onJob?(kind: string, params: Record<string, unknown>): void;
  onRelink?(media: string, path: string): void;
}) {
  const v = media.video;
  const c = media.checksum;
  return (
    <>
      <div className="section-title">Integrity</div>
      {!c && <div className="hint">No checksum yet. The content hash above is a head-and-tail fingerprint, not a verification.</div>}
      {c && (
        <dl className="props">
          <dt>sha256</dt>
          <dd title={c.sha256}>{c.sha256.slice(0, 16)}…</dd>
          <dt>last check</dt>
          <dd className={c.last?.result === "mismatch" ? "bad" : ""}>{c.last?.result ?? "—"}</dd>
        </dl>
      )}
      {onJob && (
        <div className="binding">
          <button onClick={() => onJob("media.checksum", { media: media.id })}>
            {c ? "Verify checksum" : "Establish checksum"}
          </button>
          <button disabled={!v?.decodable} onClick={() => onJob("media.thumbnail", { media: media.id, index: 0 })}>
            Make thumbnail
          </button>
        </div>
      )}
      <Resolution media={media} onJob={onJob} onRelink={onRelink} />
      <Proxies media={media} onJob={onJob} />
      <Sound media={media} onJob={onJob} />
      <div className="section-title">Media</div>
      <dl className="props">
        <dt>name</dt>
        <dd>{media.name}</dd>
        <dt>online</dt>
        <dd>{media.online === false ? "offline" : "yes"}</dd>
        <dt>container</dt>
        <dd>{sequenceOf(media) ? `image sequence · ${sequenceOf(media)}` : (media.container ?? "—")}</dd>
        <dt>size</dt>
        <dd>{media.size ? `${(media.size / 1e6).toFixed(1)} MB` : "—"}</dd>
        <dt>content hash</dt>
        <dd>{media.hash ? `${media.hash.slice(0, 16)}…` : "—"}</dd>
        <dt>path</dt>
        <dd>{media.path ?? "—"}</dd>
      </dl>
      {v && (
        <>
          <div className="section-title">Video</div>
          <dl className="props">
            <dt>codec</dt>
            <dd>{v.codec}</dd>
            <dt>size</dt>
            <dd>
              {v.width}×{v.height}
            </dd>
            <dt>pictures</dt>
            <dd>{v.frames ?? "—"}</dd>
            <dt>rate</dt>
            <dd>
              {v.rate_num && v.rate_den ? `${v.rate_num}/${v.rate_den}` : "—"}
            </dd>
            <dt>decodable</dt>
            <dd>{String(v.decodable)}</dd>
          </dl>
        </>
      )}
      <div className="section-title">Streams</div>
      <dl className="props">
        {media.streams.map((s) => (
          <Fragment key={s.id}>
            <dt>{s.name}</dt>
            <dd>
              {s.codec ?? "?"}
              {s.decodable === false ? " · no decoder" : ""}
            </dd>
          </Fragment>
        ))}
      </dl>
    </>
  );
}

/**
 * The stand-ins made from an original: what each is, and whether playback may
 * still use it. The runtime decides the state; this only shows it and asks for
 * another proxy.
 */
function Proxies({
  media,
  onJob,
}: {
  media: MediaView;
  onJob?(kind: string, params: Record<string, unknown>): void;
}) {
  const [height, setHeight] = useState(540);
  const v = media.video;
  const taller = v?.height ?? 0;
  const sizes = [1080, 720, 540, 360, 180].filter((h) => h < taller);
  return (
    <>
      <div className="section-title">Proxies</div>
      {media.proxies.length === 0 && (
        <div className="hint">No proxy of this original. Playback reads the original.</div>
      )}
      {media.proxies.map((p) => (
        <div key={p.id} className={`proxy-row ${p.state}`}>
          <span className={`render-state ${p.state === "current" ? "current" : "stale"}`}>{p.state}</span>
          <span className="label" title={p.path ?? ""}>
            {p.width}×{p.height}
            {p.sound ? " · sound" : " · no sound"}
            {p.file_size ? ` · ${(p.file_size / 1e6).toFixed(1)} MB` : ""}
          </span>
          {p.reasons.length > 0 && (
            <ul className="render-reasons">
              {p.reasons.map((why, i) => (
                <li key={i}>{why}</li>
              ))}
            </ul>
          )}
          {(p.notes ?? []).length > 0 && (
            <ul className="render-reasons">
              {(p.notes ?? []).map((note, i) => (
                <li key={i}>{note}</li>
              ))}
            </ul>
          )}
        </div>
      ))}
      {onJob && sizes.length > 0 && (
        <div className="binding">
          <label>
            Height{" "}
            <select aria-label="Proxy height" value={height} onChange={(e) => setHeight(Number(e.target.value))}>
              {sizes.map((h) => (
                <option key={h} value={h}>
                  {h}p
                </option>
              ))}
            </select>
          </label>
          <button disabled={!v?.decodable || media.online === false} onClick={() => onJob("media.proxy", { media: media.id, height })}>
            Make proxy
          </button>
        </div>
      )}
      {onJob && sizes.length === 0 && v && (
        <div className="hint">This original is {v.width}×{v.height}: a proxy would not be smaller.</div>
      )}
    </>
  );
}

/**
 * Whether this media is where the project says it is, and how to find it when
 * it is not. Identity is content: the runtime looks for *this file's content*
 * under a folder, and refuses anything that merely shares its name — so the
 * reason a likely-looking file was not accepted is shown, with its hash.
 */
function Resolution({
  media,
  onJob,
  onRelink,
}: {
  media: MediaView;
  onJob?(kind: string, params: Record<string, unknown>): void;
  onRelink?(media: string, path: string): void;
}) {
  const [folder, setFolder] = useState("");
  const [file, setFile] = useState("");
  const r = media.resolution ?? { state: null, note: null, relinked_from: null, confidence: null };
  const settled = media.online !== false && !r.state;
  return (
    <>
      <div className="section-title">Resolution</div>
      {settled && (
        <div className="hint">
          Online at its recorded path
          {r.relinked_from ? ` · relinked from ${r.relinked_from} (${r.confidence})` : ""}
        </div>
      )}
      {!settled && (
        <div className="bad" title={r.note ?? ""}>
          {r.state === "replaced"
            ? "A different file is at this path now"
            : r.state === "not_found"
              ? "Not found by content"
              : "Offline"}
          {r.note ? ` — ${r.note}` : ""}
        </div>
      )}
      {onJob && (
        <div className="binding">
          <label>
            Look in{" "}
            <input
              aria-label="Relink folder"
              placeholder="/path/to/card"
              size={22}
              value={folder}
              onChange={(e) => setFolder(e.target.value)}
            />
          </label>
          <button disabled={!folder.trim()} onClick={() => onJob("media.relink", { folder: folder.trim(), media: media.id })}>
            Find by content
          </button>
        </div>
      )}
      {onRelink && (
        <div className="binding">
          <label>
            This file{" "}
            <input
              aria-label="Relink file"
              placeholder="/path/to/A001C001.mov"
              size={22}
              value={file}
              onChange={(e) => setFile(e.target.value)}
            />
          </label>
          <button disabled={!file.trim()} onClick={() => onRelink(media.id, file.trim())}>
            Relink to it
          </button>
        </div>
      )}
    </>
  );
}

/**
 * The pattern of a media item that is an image sequence, or null. The runtime
 * describes the run in the container field; this reads the pattern out of it
 * for the one line a person wants, without inventing anything.
 */
function sequenceOf(media: MediaView): string | null {
  const c = media.container ?? "";
  const m = /pattern: "([^"]+)"/.exec(c);
  return m ? m[1] : null;
}

/**
 * The recording's envelope: whether one has been made, and whether it still
 * matches the sound. The timeline draws from it; nothing here computes one.
 */
function Sound({
  media,
  onJob,
}: {
  media: MediaView;
  onJob?(kind: string, params: Record<string, unknown>): void;
}) {
  const audio = media.streams.find((s) => s.name.startsWith("audio"));
  if (!audio) return null;
  const w = media.waveform;
  return (
    <>
      <div className="section-title">Sound</div>
      <dl className="props">
        <dt>stream</dt>
        <dd>
          {audio.codec ?? "—"}
          {audio.decodable ? "" : " · this build cannot decode it"}
        </dd>
        <dt>waveform</dt>
        <dd>
          {w ? (
            <>
              <span className={`render-state ${w.state === "current" ? "current" : "stale"}`}>{w.state}</span>
              {w.channels ? ` ${w.channels} ch` : ""}
              {w.file_size ? ` · ${(w.file_size / 1e3).toFixed(0)} kB` : ""}
            </>
          ) : (
            "none"
          )}
        </dd>
      </dl>
      {(w?.reasons ?? []).length > 0 && (
        <ul className="render-reasons">
          {(w?.reasons ?? []).map((why, i) => (
            <li key={i}>{why}</li>
          ))}
        </ul>
      )}
      {onJob && (
        <div className="binding">
          <button
            disabled={!audio.decodable || media.online === false}
            onClick={() => onJob("media.waveform", { media: media.id })}
          >
            {w ? "Make waveform again" : "Make waveform"}
          </button>
        </div>
      )}
    </>
  );
}
