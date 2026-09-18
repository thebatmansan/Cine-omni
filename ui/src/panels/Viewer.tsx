import { useCallback, useEffect, useRef, useState } from "react";
import { Capability, MediaView } from "../rpc/client";

/**
 * A real decoded picture from the project's own media.
 *
 * Every frame here came out of the real decoder: the runtime decodes it and
 * sends it as a PNG. That makes it frame-accurate and honest, and it is **not
 * real-time playback** — the verified playback path decodes into GPU textures
 * and never brings a picture back to the CPU, which a browser cannot receive.
 *
 * So this offers stepping and scrubbing and says plainly what it is. The
 * transport shows no play button it cannot honour: the capability model decides,
 * and `media.playback` is `unavailable` here.
 */
export function Viewer({
  media,
  capabilities,
  index,
  onIndex,
  title = "Viewer",
  marks,
  onMark,
  active,
  onActivate,
}: {
  media: MediaView | null;
  capabilities: Capability[];
  index: number;
  onIndex(next: number): void;
  title?: string;
  /** In and out pictures marked on this media, when this is a source monitor. */
  marks?: { in: number | null; out: number | null };
  onMark?(which: "in" | "out" | "clear"): void;
  active?: boolean;
  onActivate?(): void;
}) {
  const [loading, setLoading] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [shown, setShown] = useState<{ url: string; index: number; timecode: string | null } | null>(null);
  const [note, setNote] = useState<{ text: string; assumed: boolean } | null>(null);
  const [decodeMs, setDecodeMs] = useState<number | null>(null);
  // Looking through the project's display and view, rather than at the file's
  // own values. Scene-linear media has no display values without it.
  const [viewed, setViewed] = useState(false);
  const inflight = useRef<AbortController | null>(null);

  const playback = capabilities.find((c) => c.id === "media.playback");
  const preview = capabilities.find((c) => c.id === "media.preview");
  const frames = media?.video?.frames ?? 0;
  const decodable = media?.video?.decodable ?? false;

  const load = useCallback(
    async (want: number) => {
      if (!media || !decodable) return;
      inflight.current?.abort();
      const control = new AbortController();
      inflight.current = control;
      setLoading(true);
      try {
        const response = await fetch(`/frame?media=${media.id}&index=${want}${viewed ? "&view=project" : ""}`, {
          signal: control.signal,
        });
        if (!response.ok) {
          setFailure(await response.text());
          setShown(null);
          return;
        }
        const blob = await response.blob();
        // The camera's name for this picture, as the runtime labelled it.
        // Nothing here works out a timecode: drop-frame labelling has one
        // implementation, and it is not in the browser.
        const tc = response.headers.get("x-cine-timecode");
        setShown((old) => {
          if (old) URL.revokeObjectURL(old.url);
          return { url: URL.createObjectURL(blob), index: want, timecode: tc };
        });
        const colour = response.headers.get("x-cine-colour");
        setNote(
          colour
            ? { text: decodeHeader(colour), assumed: response.headers.get("x-cine-colour-assumed") === "true" }
            : null,
        );
        const ms = response.headers.get("x-cine-decode-ms");
        setDecodeMs(ms ? Number(ms) : null);
        setFailure(null);
      } catch (e) {
        if ((e as Error).name !== "AbortError") setFailure(String(e));
      } finally {
        setLoading(false);
      }
    },
    [media, decodable, viewed],
  );

  useEffect(() => {
    void load(index);
  }, [index, load]);

  useEffect(() => () => inflight.current?.abort(), []);

  const step = (by: number) => onIndex(Math.max(0, Math.min(frames - 1, index + by)));

  return (
    <div className={`panel viewer${active ? " active" : ""}`} onMouseDown={onActivate}>
      <div className="panel-head">
        <span>{title}</span>
        {media && <span className="count">{media.name}</span>}
        <span className="spacer" />
        {decodeMs !== null && <span className="count">decode {decodeMs.toFixed(0)} ms</span>}
        <button
          aria-pressed={viewed}
          title="Look through the project's display and view, as a render does. Scene-linear media has no display values without it."
          onMouseDown={(e) => e.stopPropagation()}
          onClick={() => setViewed((v) => !v)}
        >
          View
        </button>
      </div>

      <div className="viewer-stage">
        {!media && <div className="viewer-empty">Select media in the project browser.</div>}
        {media && !decodable && (
          <div className="viewer-empty">
            This build cannot decode {media.video?.codec ?? "this media"}.
            <br />
            No picture is shown rather than a blank one.
          </div>
        )}
        {failure && <div className="viewer-empty">{failure}</div>}
        {shown && <img src={shown.url} alt={`picture ${shown.index}`} />}
        {note && (
          <div className={`viewer-note${note.assumed ? " assumed" : ""}`}>
            {note.text}
            {note.assumed && " — assumed, not declared by the file"}
          </div>
        )}
      </div>

      <div className="transport">
        <button onClick={() => onIndex(0)} disabled={!decodable} title="First picture (Home)">
          ⏮
        </button>
        <button onClick={() => step(-1)} disabled={!decodable || index <= 0} title="Step back (←)">
          ◀
        </button>
        <button
          onClick={() => step(1)}
          disabled={!decodable || index >= frames - 1}
          title="Step forward (→)"
        >
          ▶
        </button>
        {/* The source timecode where the file has one, and the picture number
            either way: one is what the camera called this frame, the other is
            where it sits in the file, and they are not interchangeable. */}
        <span className="tc" title={shown?.timecode ? "source timecode" : "picture number"}>
          {shown ? (shown.timecode ?? `#${shown.index}`) : "—"}
        </span>
        <span className="of">{shown?.timecode ? `#${shown.index} / ${frames || "—"}` : `/ ${frames || "—"}`}</span>
        <input
          type="range"
          min={0}
          max={Math.max(0, frames - 1)}
          value={index}
          disabled={!decodable}
          onChange={(e) => onIndex(Number(e.target.value))}
          aria-label="picture"
        />
        {onMark && (
          <>
            <button onClick={() => onMark("in")} disabled={!decodable} title="Mark in (I)">
              In{marks?.in != null ? ` #${marks.in}` : ""}
            </button>
            <button onClick={() => onMark("out")} disabled={!decodable} title="Mark out (O)">
              Out{marks?.out != null ? ` #${marks.out}` : ""}
            </button>
          </>
        )}
        <span className="spacer" />
        {loading && <span className="of">decoding…</span>}
        {/* The play control that does not exist says why, rather than being a
            button that does nothing. */}
        <span
          className="of"
          title={playback?.detail ?? ""}
          style={{ color: "var(--text-faint)" }}
        >
          {playback?.state === "unavailable" ? "no real-time playback here" : ""}
        </span>
      </div>

      {preview && !onMark && (
        <div className="notice">
          <strong>Frame-accurate preview, not playback</strong>
          {preview.detail}
          {playback && <> {playback.detail}.</>}
        </div>
      )}
    </div>
  );
}

/** A header value the runtime percent-encoded; shown as is if it was not. */
function decodeHeader(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}
