import { useState } from "react";
import { MediaView, StructureView, TimelineSummary } from "../rpc/client";

/**
 * The project, as the graph holds it.
 *
 * Sequences, the production chain, and media. The chain is drawn from the
 * `structure` projection, which reads real edges: a shot appears under a scene
 * because the graph says it derives from it, never because their names look
 * alike. When a scene has no shots broken down, it says so.
 */
export function Browser({
  timelines,
  structure,
  media,
  selectedTimeline,
  selectedMedia,
  onTimeline,
  onMedia,
}: {
  timelines: TimelineSummary[];
  structure: StructureView | null;
  media: MediaView[];
  selectedTimeline: string | null;
  selectedMedia: string | null;
  onTimeline(id: string): void;
  onMedia(id: string): void;
}) {
  const [open, setOpen] = useState({ sequences: true, script: true, media: true });
  const toggle = (k: keyof typeof open) => setOpen((o) => ({ ...o, [k]: !o[k] }));

  return (
    <div className="panel browser">
      <div className="panel-head">
        <span>Project</span>
        <span className="count">
          {timelines.length} seq · {media.length} media
        </span>
      </div>
      <div className="panel-body tree">
        <Group label="Sequences" open={open.sequences} onToggle={() => toggle("sequences")}>
          {timelines.length === 0 && <div className="empty">No sequence imported.</div>}
          {timelines.map((t) => (
            <div
              key={t.id}
              className="tree-row depth-1"
              aria-selected={t.id === selectedTimeline}
              onMouseDown={() => onTimeline(t.id)}
            >
              <span className="kind">SEQ</span>
              <span className="label">{t.name}</span>
              <span className="meta">{t.tracks} tracks</span>
            </div>
          ))}
        </Group>

        <Group label="Script" open={open.script} onToggle={() => toggle("script")}>
          {!structure?.scenes.length && <div className="empty">No screenplay imported.</div>}
          {structure?.scenes.map((scene) => (
            <div key={scene.id}>
              <div className="tree-row depth-1" title={scene.heading ?? ""}>
                <span className="kind">SC</span>
                <span className="label">
                  {scene.number ? `${scene.number}. ` : ""}
                  {scene.heading ?? "untitled scene"}
                </span>
                <span className="meta">
                  {scene.eighths != null ? `${scene.eighths}/8` : ""}
                </span>
              </div>
              {scene.shots.length === 0 && (
                <div className="tree-row depth-2">
                  <span className="label" style={{ color: "var(--text-faint)" }}>
                    no shots broken down
                  </span>
                </div>
              )}
              {scene.shots.map((shot) => (
                <div key={shot.id}>
                  <div className="tree-row depth-2">
                    <span className="kind">SH</span>
                    <span className="label">{shot.name}</span>
                    <span className="meta">{shot.takes.length} takes</span>
                  </div>
                  {shot.takes.map((take) => (
                    <div className="tree-row depth-3" key={take.id}>
                      <span className="kind">TK</span>
                      <span className="label">{take.name}</span>
                      <span className="meta">{take.media.length ? "media" : "—"}</span>
                    </div>
                  ))}
                </div>
              ))}
            </div>
          ))}
        </Group>

        <Group label="Media" open={open.media} onToggle={() => toggle("media")}>
          {media.length === 0 && <div className="empty">No media in this project.</div>}
          {media.map((m) => (
            <div
              key={m.id}
              className="tree-row depth-1"
              aria-selected={m.id === selectedMedia}
              onMouseDown={() => onMedia(m.id)}
              title={m.path ?? ""}
            >
              <span className="kind">{m.video ? "V" : "A"}</span>
              <span className="label">{m.name}</span>
              <span className="meta">
                {m.online === false ? "offline" : m.video?.frames ? `${m.video.frames}f` : ""}
              </span>
            </div>
          ))}
        </Group>
      </div>
    </div>
  );
}

function Group({
  label,
  open,
  onToggle,
  children,
}: {
  label: string;
  open: boolean;
  onToggle(): void;
  children: React.ReactNode;
}) {
  return (
    <>
      <div className="tree-row" onMouseDown={onToggle}>
        <span className="twisty">{open ? "▾" : "▸"}</span>
        <span className="label" style={{ letterSpacing: "0.06em", fontSize: 10, textTransform: "uppercase", color: "var(--text-dim)" }}>
          {label}
        </span>
      </div>
      {open && children}
    </>
  );
}
