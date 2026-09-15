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
}: {
  media: MediaView | null;
  capabilities: Capability[];
  index: number;
  onIndex(next: number): void;
}) {
  const [loading, setLoading] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [shown, setShown] = useState<{ url: string; index: number } | null>(null);
  const [note, setNote] = useState<{ text: string; assumed: boolean } | null>(null);
  const [decodeMs, setDecodeMs] = useState<number | null>(null);
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
        const response = await fetch(`/frame?media=${media.id}&index=${want}`, {
          signal: control.signal,
        });
        if (!response.ok) {
          setFailure(await response.text());
          setShown(null);
          return;
        }
        const blob = await response.blob();
        setShown((old) => {
          if (old) URL.revokeObjectURL(old.url);
          return { url: URL.createObjectURL(blob), index: want };
        });
        const colour = response.headers.get("x-cine-colour");
        setNote(
          colour
            ? { text: colour, assumed: response.headers.get("x-cine-colour-assumed") === "true" }
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
    [media, decodable],
  );

  useEffect(() => {
    void load(index);
  }, [index, load]);

  useEffect(() => () => inflight.current?.abort(), []);

  const step = (by: number) => onIndex(Math.max(0, Math.min(frames - 1, index + by)));

  return (
    <div className="panel viewer">
      <div className="panel-head">
        <span>Viewer</span>
        {media && <span className="count">{media.name}</span>}
        <span className="spacer" />
        {decodeMs !== null && <span className="count">decode {decodeMs.toFixed(0)} ms</span>}
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
        <span className="tc">{shown ? `#${shown.index}` : "—"}</span>
        <span className="of">/ {frames || "—"}</span>
        <input
          type="range"
          min={0}
          max={Math.max(0, frames - 1)}
          value={index}
          disabled={!decodable}
          onChange={(e) => onIndex(Number(e.target.value))}
          aria-label="picture"
        />
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

      {preview && (
        <div className="notice">
          <strong>Frame-accurate preview, not playback</strong>
          {preview.detail}
          {playback && <> {playback.detail}.</>}
        </div>
      )}
    </div>
  );
}
