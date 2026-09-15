import { Fragment } from "react";
import { MediaView, TimelineItem, TimelineTrack, TimelineView } from "../rpc/client";

/** Whatever is selected, in detail. Values come from the projection unchanged. */
export function Inspector({
  timeline,
  selection,
  media,
}: {
  timeline: TimelineView | null;
  selection: { item: TimelineItem; track: TimelineTrack } | null;
  media: MediaView | null;
}) {
  return (
    <div className="panel inspector">
      <div className="panel-head">
        <span>Inspector</span>
        <span className="spacer" />
        <span className="count">{selection ? selection.item.kind : media ? "media" : "—"}</span>
      </div>
      <div className="panel-body pad">
        {selection && <ClipDetail item={selection.item} track={selection.track} />}
        {!selection && media && <MediaDetail media={media} />}
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

function MediaDetail({ media }: { media: MediaView }) {
  const v = media.video;
  return (
    <>
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
