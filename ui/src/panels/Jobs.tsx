import { useEffect, useState } from "react";
import {
  DEFAULT_RENDER,
  parseSize,
  renderFileUrl,
  renderFrameUrl,
  renderParams,
  type FrameMarks,
  type RenderFit,
  type RenderLook,
  type RenderOptions,
  type RenderOutput,
} from "../editing/edit";
import type { FailedRender, JobView, RenderView, RpcClient } from "../rpc/client";

/**
 * Background work, as the runtime's job system reports it.
 *
 * Every row is a job record pushed by the runtime: state, progress, attempts,
 * what it produced or why it failed. Nothing here estimates or animates
 * progress the runner did not report.
 */
export function Jobs({
  jobs,
  onCancel,
  onRetry,
  render,
}: {
  jobs: JobView[];
  onCancel(id: number): void;
  onRetry(id: number): void;
  /** The open sequence, when there is one to render. */
  render?: {
    client: RpcClient;
    revision: number;
    timeline: { id: string; name: string };
    languages: string[];
    marks: FrameMarks;
    onRender(params: Record<string, unknown>): void;
  };
}) {
  const active = jobs.filter((j) => j.state === "queued" || j.state === "running").length;
  return (
    <div className="panel jobs">
      <div className="panel-head">
        <span>Jobs</span>
        <span className="count">{active} active</span>
        <span className="spacer" />
        <span className="count">{jobs.length} total</span>
      </div>
      <div className="panel-body">
        {render && <RenderForm {...render} />}
        {render && <Renders client={render.client} timeline={render.timeline.id} revision={render.revision} jobs={jobs} />}
        {jobs.length === 0 && <div className="empty">No background work in this session.</div>}
        {jobs.map((j) => (
          <div key={j.id} className={`job ${j.state}`}>
            <div className="job-line">
              <span className={`job-state ${j.state}`}>{j.state}</span>
              <span className="label" title={j.kind}>{j.label}</span>
              <span className="meta">{j.resource}</span>
              {(j.state === "queued" || j.state === "running") && (
                <button onClick={() => onCancel(j.id)}>Cancel</button>
              )}
              {(j.state === "failed" || j.state === "cancelled") && (
                <button onClick={() => onRetry(j.id)}>Retry</button>
              )}
            </div>
            {j.state === "running" && (
              <div className="job-progress" title={j.message ?? ""}>
                <div style={{ width: `${Math.round((j.progress ?? 0) * 100)}%` }} />
              </div>
            )}
            <div className="job-detail">
              {j.message && j.state !== "succeeded" && <span>{j.message}</span>}
              {j.error && <span className="bad">{j.error}</span>}
              {j.state === "succeeded" && summary(j)}
              {j.attempts > 1 && <span>attempt {j.attempts} of {j.max_attempts}</span>}
              {j.resumed && <span>resumed after restart</span>}
              {j.elapsed_ms != null && j.state !== "queued" && <span>{(j.elapsed_ms / 1000).toFixed(2)} s</span>}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function summary(j: JobView) {
  const o = j.output ?? {};
  if (j.kind === "media.checksum") {
    const result = String(o.result ?? "");
    return <span className={result === "mismatch" ? "bad" : "good"}>{result}{o.sha256 ? ` · sha256 ${String(o.sha256).slice(0, 12)}…` : ""}</span>;
  }
  if (j.kind === "media.thumbnail") return <span>{o.cached ? "already cached" : `${o.width}×${o.height}`}</span>;
  if (j.kind === "render.sequence") {
    if (o.cancelled) return <span>cancelled; nothing kept</span>;
    const look = (o.look ?? {}) as Record<string, unknown>;
    const sound = o.sound as Record<string, unknown> | null | undefined;
    return (
      <span title={String(o.path ?? "")}>
        {String(o.output ?? "png8")} · {String(o.frames)} frames · {String(o.width)}×{String(o.height)} ·{" "}
        {look.kind === "project_view" ? `${String(look.display)} / ${String(look.view)}` : "as decoded"}
        {sound ? ` · sound ${String(sound.sample_rate)} Hz` : ""}
        {o.captions_burned != null ? ` · captions on ${String(o.captions_burned)} frames` : ""}
      </span>
    );
  }
  return null;
}

/**
 * Render the open sequence. The form only chooses; the runtime resolves the
 * cut, refuses what it cannot render faithfully, and reports where it wrote.
 * Nothing here decides whether a combination is possible — the runtime's
 * refusal, shown on the job, is the answer.
 */
function RenderForm({
  timeline,
  languages,
  marks,
  onRender,
}: {
  timeline: { id: string; name: string };
  languages: string[];
  marks: FrameMarks;
  onRender(params: Record<string, unknown>): void;
}) {
  const [options, setOptions] = useState<RenderOptions>(DEFAULT_RENDER);
  const [sizeText, setSizeText] = useState("");
  const size = sizeText.trim() === "" ? null : parseSize(sizeText);
  const sizeInvalid = sizeText.trim() !== "" && size === null;
  const ranged = marks.in != null && marks.out != null && marks.out >= marks.in;
  const set = (patch: Partial<RenderOptions>) => setOptions((o) => ({ ...o, ...patch }));
  return (
    <div className="render-form" aria-label="Render">
      <span className="label">Render “{timeline.name}”</span>
      <label>
        Output{" "}
        <select aria-label="Output" value={options.output} onChange={(e) => set({ output: e.target.value as RenderOutput })}>
          <option value="png8">PNG frames, 8-bit</option>
          <option value="png16">PNG frames, 16-bit</option>
          <option value="movie">Movie: H.264 + 24-bit PCM (.mov)</option>
        </select>
      </label>
      {options.output === "movie" && (
        <label>
          Sound{" "}
          <select aria-label="Sound" value={options.sound} onChange={(e) => set({ sound: e.target.value as RenderOptions["sound"] })}>
            <option value="auto">The cut's sound, if it has any</option>
            <option value="on">Required</option>
            <option value="off">None</option>
          </select>
        </label>
      )}
      <label>
        Look{" "}
        <select aria-label="Look" value={options.look} onChange={(e) => set({ look: e.target.value as RenderLook })}>
          <option value="as_decoded">As decoded</option>
          <option value="project_view">Through the project view</option>
        </select>
      </label>
      <label>
        Burn in{" "}
        <select aria-label="Burn in" value={options.captions ?? ""} onChange={(e) => set({ captions: e.target.value || null })}>
          <option value="">No captions</option>
          {languages.map((l) => (
            <option key={l} value={l}>
              {l} captions
            </option>
          ))}
        </select>
      </label>
      <label>
        Size{" "}
        <input
          aria-label="Size"
          className={sizeInvalid ? "bad" : ""}
          placeholder="source"
          size={10}
          value={sizeText}
          onChange={(e) => setSizeText(e.target.value)}
        />
      </label>
      {size && (
        <label>
          Fit{" "}
          <select aria-label="Fit" value={options.fit} onChange={(e) => set({ fit: e.target.value as RenderFit })}>
            <option value="refuse">Refuse a different shape</option>
            <option value="letterbox">Letterbox</option>
            <option value="crop">Crop</option>
            <option value="stretch">Stretch</option>
          </select>
        </label>
      )}
      <span className="meta">{ranged ? `marked frames ${marks.in}–${marks.out}` : "whole cut"}</span>
      <button disabled={sizeInvalid} onClick={() => onRender(renderParams(timeline.id, { ...options, size }, marks))}>
        Render
      </button>
    </div>
  );
}

/**
 * Renders of the open sequence on disk, and whether each still matches the
 * project. The state is the runtime's comparison of what each render
 * depended on with the project as it is now; nothing here judges it.
 */
function Renders({
  client,
  timeline,
  revision,
  jobs,
}: {
  client: RpcClient;
  timeline: string;
  revision: number;
  jobs: JobView[];
}) {
  const [renders, setRenders] = useState<RenderView[] | null>(null);
  const [failed, setFailed] = useState<FailedRender[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  // A render finishing or failing changes the list; so does any project change.
  const finished = jobs.filter((j) => j.kind === "render.sequence" && (j.state === "succeeded" || j.state === "failed")).length;
  useEffect(() => {
    let live = true;
    client
      .renders(timeline)
      .then((r) => {
        if (!live) return;
        setRenders(r.renders);
        setFailed(r.failed);
        setError(null);
      })
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [client, timeline, revision, finished]);

  if (error) return <div className="renders"><div className="bad">{error}</div></div>;
  if (!renders) return null;
  return (
    <div className="renders" aria-label="Renders">
      <div className="panel-subhead">Renders of this sequence</div>
      {renders.length === 0 && failed.length === 0 && <div className="empty">No renders of this sequence yet.</div>}
      {renders.map((r) => (
        <div key={r.name} className={`render-row ${r.state}`}>
          <div className="job-line">
            <span className={`render-state ${r.state}`}>{r.state}</span>
            <span className="label" title={r.path}>
              {r.output ?? "?"} · {r.frames?.count ?? "?"} frames · {r.width}×{r.height}
              {r.look?.kind === "project_view" ? ` · ${r.look.display} / ${r.look.view}` : " · as decoded"}
              {r.sound ? ` · sound` : ""}
              {r.captions ? ` · ${r.captions.language} captions` : ""}
            </span>
            <button onClick={() => setPreview(preview === r.name ? null : r.name)}>{preview === r.name ? "Hide" : "Preview"}</button>
            {r.movie ? (
              <a className="button" href={renderFileUrl(timeline, r.name, "movie.mov")} download>
                Movie
              </a>
            ) : null}
            <a className="button" href={renderFileUrl(timeline, r.name, "manifest.json")} target="_blank" rel="noreferrer">
              Manifest
            </a>
          </div>
          {r.reasons.length > 0 && (
            <ul className="render-reasons">
              {r.reasons.map((why, i) => (
                <li key={i}>{why}</li>
              ))}
            </ul>
          )}
          {/* True of the render without making it out of date: the runtime
              separates the two, and so does this. */}
          {r.notes?.length > 0 && (
            <ul className="render-notes">
              {r.notes.map((note, i) => (
                <li key={i}>{note}</li>
              ))}
            </ul>
          )}
          {r.remakeable === false && (
            <div className="render-notes">This render cannot be made again until its media is found.</div>
          )}
          {r.sound && r.sound.clipped_samples > 0 && (
            <div className="bad">{r.sound.clipped_samples} sound samples clipped at full scale</div>
          )}
          {preview === r.name && r.frames && (
            <img
              className="render-preview"
              alt={`middle picture of ${r.name}`}
              src={renderFrameUrl(timeline, r.name, r.movie ? Math.floor(r.frames.count / 2) : r.frames.first + Math.floor(r.frames.count / 2))}
            />
          )}
        </div>
      ))}
      {failed.map((f) => (
        <div key={`job-${f.job}`} className="render-row failed">
          <div className="job-line">
            <span className="render-state failed">failed</span>
            <span className="label">{f.label}</span>
          </div>
          <ul className="render-reasons">
            {f.reasons.filter(Boolean).map((why, i) => (
              <li key={i}>{why}</li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}
