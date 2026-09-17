import { useState } from "react";
import { renderParams, type FrameMarks, type RenderLook } from "../editing/edit";
import type { JobView } from "../rpc/client";

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
    return (
      <span title={String(o.path ?? "")}>
        {String(o.frames)} frames · {String(o.width)}×{String(o.height)} ·{" "}
        {look.kind === "project_view" ? `${String(look.display)} / ${String(look.view)}` : "as decoded"}
        {o.captions_burned != null ? ` · captions on ${String(o.captions_burned)} frames` : ""} · {String(o.path ?? "")}
      </span>
    );
  }
  return null;
}

/**
 * Render the open sequence. The form only chooses; the runtime resolves the
 * cut, refuses what it cannot render faithfully, and reports where it wrote.
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
  const [look, setLook] = useState<RenderLook>("as_decoded");
  const [captions, setCaptions] = useState<string>("");
  const ranged = marks.in != null && marks.out != null && marks.out >= marks.in;
  return (
    <div className="render-form" aria-label="Render">
      <span className="label">Render “{timeline.name}”</span>
      <label>
        Look{" "}
        <select value={look} onChange={(e) => setLook(e.target.value as RenderLook)}>
          <option value="as_decoded">As decoded</option>
          <option value="project_view">Through the project view</option>
        </select>
      </label>
      <label>
        Burn in{" "}
        <select value={captions} onChange={(e) => setCaptions(e.target.value)}>
          <option value="">No captions</option>
          {languages.map((l) => (
            <option key={l} value={l}>
              {l} captions
            </option>
          ))}
        </select>
      </label>
      <span className="meta">{ranged ? `marked frames ${marks.in}–${marks.out}` : "whole cut"} · PNG sequence</span>
      <button onClick={() => onRender(renderParams(timeline.id, look, captions || null, marks))}>Render</button>
    </div>
  );
}
