import { useCallback, useEffect, useState } from "react";
import { newCaptionSpan } from "../editing/edit";
import type {
  CaptionCandidate,
  CaptionFormat,
  CaptionsView,
  CaptionView,
  ExactTime,
  RpcClient,
  TimelineView,
} from "../rpc/client";

/**
 * A sequence's captions, per language.
 *
 * Every row is the runtime's: its times are the sequence's exact frames, and
 * its issues are the runtime's check against delivery rules — nothing here
 * measures a gap or counts characters a second time. Typing in a caption is a
 * draft until it leaves the field; then it is one commit, which every other
 * window sees and undo takes back.
 *
 * New captions go where the record marks are, or two seconds from the
 * playhead when nothing is marked, and the runtime refuses one that ends
 * before it starts.
 */
export function Captions({
  client,
  timeline,
  view,
  language,
  onLanguage,
  playhead,
  marks,
  onSeek,
  run,
}: {
  client: RpcClient;
  timeline: TimelineView | null;
  /** The shared caption state (`useCaptions`). */
  view: CaptionsView | null;
  language: string | null;
  onLanguage(l: string | null): void;
  playhead: ExactTime | null;
  /** Record marks in ticks at the sequence rate, as the timeline holds them. */
  marks: { in: number | null; out: number | null };
  onSeek(ticks: number): void;
  run<T>(label: string, call: () => Promise<T>): Promise<T | null>;
}) {
  const setLanguage = onLanguage;
  const [newLanguage, setNewLanguage] = useState("");
  const [importing, setImporting] = useState(false);
  const [srt, setSrt] = useState("");
  const [note, setNote] = useState<string | null>(null);

  const id = timeline?.id ?? null;

  const add = useCallback(async () => {
    if (!id || !playhead || !language) return;
    const rate = { num: playhead.rate_num, den: playhead.rate_den };
    const at = (ticks: number): ExactTime => ({ ticks, rate_num: rate.num, rate_den: rate.den });
    const { start, end } = newCaptionSpan(marks, playhead.ticks, rate);
    await run("add caption", () => client.setCaption(id, language, { start: at(start), end: at(end), text: "…" }));
  }, [client, id, language, marks.in, marks.out, playhead, run]);

  const download = useCallback(
    async (format: CaptionFormat) => {
      if (!id || !language) return;
      const out = await run(`export ${format}`, () => client.exportCaptions(id, language, format));
      if (!out) return;
      const url = URL.createObjectURL(new Blob([out.text], { type: out.mime }));
      const a = document.createElement("a");
      a.href = url;
      a.download = `${timeline?.name ?? "captions"}.${language}.${out.extension}`;
      a.click();
      URL.revokeObjectURL(url);
    },
    [client, id, language, run, timeline?.name],
  );

  if (!timeline) return <div className="empty">Open a sequence to caption it.</div>;
  const track = view?.track ?? null;
  const languages = view?.languages ?? [];

  return (
    <div className="captions">
      <div className="panel-body pad">
        <div className="row">
          <select
            aria-label="caption language"
            value={language ?? ""}
            onChange={(e) => setLanguage(e.target.value || null)}
          >
            {languages.length === 0 && <option value="">no captions yet</option>}
            {languages.map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
            {language && !languages.includes(language) && <option value={language}>{language} (new)</option>}
          </select>
          <input
            placeholder="new language, e.g. en-GB"
            aria-label="new caption language"
            value={newLanguage}
            onChange={(e) => setNewLanguage(e.target.value)}
          />
          <button
            disabled={!/^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$/.test(newLanguage.trim())}
            onClick={() => {
              setLanguage(newLanguage.trim());
              setNewLanguage("");
            }}
          >
            Use
          </button>
        </div>

        {language && (
          <div className="row">
            <button onClick={() => void add()} disabled={!playhead}>
              Add at {marks.in != null ? "marks" : "playhead"}
            </button>
            <button aria-pressed={importing} onClick={() => setImporting((v) => !v)}>
              Import SRT…
            </button>
            <button
              title="Link each caption to the line of dialogue it reads exactly, among the scenes under it. Ambiguous ones are left alone."
              disabled={!track?.captions.length}
              onClick={async () => {
                const r = await run("link captions", () => client.linkCaptionsExact(timeline.id, language));
                if (r) setNote(`${r.linked} caption${r.linked === 1 ? "" : "s"} linked to the script`);
              }}
            >
              Link to script
            </button>
            <span className="spacer" />
            {(["imsc", "srt", "vtt"] as CaptionFormat[]).map((f) => (
              <button key={f} disabled={!track?.captions.length} onClick={() => void download(f)}>
                {f.toUpperCase()}
              </button>
            ))}
          </div>
        )}

        {importing && language && (
          <div className="import">
            <textarea
              rows={6}
              placeholder="Paste SubRip text. It replaces this language's captions; times are moved onto the sequence's frames and anything that moved is reported."
              value={srt}
              onChange={(e) => setSrt(e.target.value)}
            />
            <button
              disabled={!srt.trim()}
              onClick={async () => {
                const r = await run("import captions", () => client.importCaptions(timeline.id, language, srt));
                if (!r) return;
                const { report } = r;
                setNote(
                  [
                    `${report.captions} read`,
                    report.snapped ? `${report.snapped} moved onto a frame (up to ${report.largest_snap_ms.toFixed(1)} ms)` : null,
                    report.skipped.length ? `${report.skipped.length} skipped: ${report.skipped.join("; ")}` : null,
                  ]
                    .filter(Boolean)
                    .join(" · "),
                );
                setSrt("");
                setImporting(false);
              }}
            >
              Replace with this
            </button>
          </div>
        )}
        {note && <div className="note">{note}</div>}

        {track && (
          <div className="section-title">
            {track.captions.length} captions ·{" "}
            {track.issue_count === 0 ? "no delivery issues" : `${track.issue_count} delivery issues`}
            {track.captions.some((c) => c.review) &&
              ` · ${track.captions.filter((c) => c.review).length} to review against the script`}
          </div>
        )}
        {track?.captions.map((c) => (
          <CaptionRow
            key={`${c.index}:${c.record.start.ticks}`}
            caption={c}
            onSeek={() => onSeek(c.record.start.ticks)}
            onSave={(text) =>
              void run("edit caption", () =>
                client.setCaption(timeline.id, track.language, {
                  index: c.index,
                  start: c.record.start,
                  end: c.record.end,
                  text,
                  placement: c.placement,
                  speaker: c.speaker ?? undefined,
                }),
              )
            }
            onPlacement={(placement) =>
              void run("move caption", () =>
                client.setCaption(timeline.id, track.language, {
                  index: c.index,
                  start: c.record.start,
                  end: c.record.end,
                  text: c.lines.join("\n"),
                  placement,
                  speaker: c.speaker ?? undefined,
                }),
              )
            }
            onRemove={() => void run("remove caption", () => client.removeCaption(timeline.id, track.language, c.index))}
            candidates={() =>
              c.key
                ? client.captionCandidates(timeline.id, track.language, c.key).then((r) => r.candidates)
                : Promise.resolve([])
            }
            onLink={(line) =>
              c.key && void run("link caption", () => client.linkCaption(timeline.id, track.language, c.key!, line))
            }
            onUnlink={() =>
              c.key && void run("unlink caption", () => client.unlinkCaption(timeline.id, track.language, c.key!))
            }
            onAccept={() =>
              c.key &&
              void run("accept rewritten line", () => client.acceptCaptionLine(timeline.id, track.language, c.key!))
            }
          />
        ))}
      </div>
    </div>
  );
}

function CaptionRow({
  caption,
  onSeek,
  onSave,
  onPlacement,
  onRemove,
  candidates,
  onLink,
  onUnlink,
  onAccept,
}: {
  caption: CaptionView;
  onSeek(): void;
  onSave(text: string): void;
  onPlacement(p: "top" | "bottom"): void;
  onRemove(): void;
  candidates(): Promise<CaptionCandidate[]>;
  onLink(line: string): void;
  onUnlink(): void;
  onAccept(): void;
}) {
  const [picking, setPicking] = useState<CaptionCandidate[] | null>(null);
  const committed = caption.lines.join("\n");
  const [draft, setDraft] = useState(committed);
  // A change from anywhere else replaces the draft: the runtime's text wins.
  useEffect(() => setDraft(committed), [committed]);

  return (
    <div className={`caption${caption.issues.length ? " has-issues" : ""}`}>
      <div className="caption-line">
        <button className="link" title="Move the playhead here" onClick={onSeek}>
          {caption.record.start.timecode}
        </button>
        <span className="meta">→ {caption.record.end.timecode}</span>
        {caption.speaker && <span className="meta">{caption.speaker}</span>}
        <span className="spacer" />
        <button
          aria-pressed={caption.placement === "top"}
          title="Move the caption to the top of the picture"
          onClick={() => onPlacement(caption.placement === "top" ? "bottom" : "top")}
        >
          Top
        </button>
        <button onClick={onRemove} aria-label="remove caption">
          ×
        </button>
      </div>
      <textarea
        rows={Math.max(1, draft.split("\n").length)}
        value={draft}
        aria-label={`caption at ${caption.record.start.timecode}`}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => draft !== committed && draft.trim() && onSave(draft)}
      />
      {caption.issues.map((i, k) => (
        <div key={k} className="bad issue">
          {i.message}
        </div>
      ))}
      {caption.review && (
        <div className="review">
          <span className="bad">{caption.review}</span>
          {caption.link?.now != null && (
            <button title="Take the line's current words as the transcription" onClick={onAccept}>
              Accept
            </button>
          )}
          <button onClick={onUnlink}>Unlink</button>
        </div>
      )}
      {caption.link ? (
        <div className="linked" title={caption.link.heading ?? ""}>
          <span className="meta">
            transcribes {caption.link.character || "a line"} in {caption.link.heading ?? "a scene"}
          </span>
          {!caption.review && (
            <button className="link" onClick={onUnlink}>
              unlink
            </button>
          )}
        </div>
      ) : (
        caption.key && (
          <div className="linked">
            <button
              className="link"
              onClick={async () => setPicking(picking ? null : await candidates())}
            >
              {picking ? "cancel" : "link to a line…"}
            </button>
          </div>
        )
      )}
      {picking && (
        <div className="candidates">
          {picking.length === 0 && (
            <div className="note">
              No scene is bound to the picture under this caption, so there is no dialogue to link it to.
            </div>
          )}
          {picking.map((k) => (
            <button
              key={k.line.id}
              className="candidate"
              onClick={() => {
                setPicking(null);
                onLink(k.line.id);
              }}
            >
              <span className="meta">{k.line.character}</span> {k.line.text}
              <span className="meta"> · {k.heading}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
