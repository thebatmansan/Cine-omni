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
}: {
  jobs: JobView[];
  onCancel(id: number): void;
  onRetry(id: number): void;
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
  return null;
}
