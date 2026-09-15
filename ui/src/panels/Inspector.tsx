import { Fragment } from "react";
import { useState } from "react";
import { MediaView, ScriptView, TimelineItem, TimelineTrack, TimelineView } from "../rpc/client";

/** Whatever is selected, in detail. Values come from the projection unchanged. */
export function Inspector({
  timeline,
  selection,
  media,
  script,
  onConformDone,
  onBind,
  onUnbind,
  onJob,
}: {
  timeline: TimelineView | null;
  selection: { item: TimelineItem; track: TimelineTrack } | null;
  media: MediaView | null;
  script?: ScriptView | null;
  onConformDone?(clip: string): void;
  onBind?(clip: string, scene: string): void;
  onUnbind?(clip: string, scene: string): void;
  onJob?(kind: string, params: Record<string, unknown>): void;
}) {
  return (
    <div className="inspector">
      <div className="panel-body pad">
        {selection && (
          <>
            <ClipDetail item={selection.item} track={selection.track} />
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
        {!selection && media && <MediaDetail media={media} onJob={onJob} />}
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
}: {
  media: MediaView;
  onJob?(kind: string, params: Record<string, unknown>): void;
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
      <div className="section-title">Media</div>
      <dl className="props">
        <dt>name</dt>
        <dd>{media.name}</dd>
        <dt>online</dt>
        <dd>{media.online === false ? "offline" : "yes"}</dd>
        <dt>container</dt>
        <dd>{media.container ?? "—"}</dd>
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
