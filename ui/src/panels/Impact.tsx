import { useState } from "react";
import type { HistoryView, ImpactView } from "../rpc/client";

/**
 * What the last change did, across the production.
 *
 * Read straight from the runtime's `impact` projection, which is built from the
 * deltas the engine committed — the change itself and every reactor
 * transaction it caused. This panel adds no consequences of its own. When a
 * change touched only the cut, it says only the cut; when a script change
 * reached material bound to the scene, the clips and the reactor's reason are
 * named here.
 */
export function Impact({
  impact,
  onReveal,
}: {
  impact: ImpactView | null;
  onReveal?(entity: string): void;
}) {
  const [open, setOpen] = useState<string | null>(null);
  return (
    <div className="panel impact">
      <div className="panel-head">
        <span>Impact</span>
        {impact && <span className="count">{impact.label}</span>}
        <span className="spacer" />
        {impact && (
          <span className="count">
            rev {impact.revisions[0]}
            {impact.revisions.length > 1 ? `–${impact.revisions[impact.revisions.length - 1]}` : ""}
          </span>
        )}
      </div>
      <div className="panel-body">
        {!impact && <div className="empty">No change yet in this session.</div>}
        {impact && (
          <>
            {impact.conform.length > 0 && (
              <div className="impact-conform">
                <div className="impact-conform-title">
                  {impact.conform.length} clip{impact.conform.length === 1 ? "" : "s"} need conforming
                </div>
                {impact.conform.map((c) => (
                  <button key={c.clip} className="impact-row conform" onClick={() => onReveal?.(c.clip)}>
                    <span className="label">{c.name ?? c.clip}</span>
                    <span className="reason">{c.reason}</span>
                  </button>
                ))}
              </div>
            )}
            <div className="impact-steps">
              {impact.steps.map((s) => (
                <div key={s.revision} className="impact-step" style={{ paddingLeft: 8 + s.depth * 12 }}>
                  <span className="mono">{s.revision}</span>
                  <span className={`by${s.by === "user" ? " user" : ""}`}>{s.by}</span>
                  <span className="label">{s.action}</span>
                  <span className="meta">{s.ops} ops</span>
                </div>
              ))}
            </div>
            {impact.departments.map((d) => (
              <div key={d.department} className="impact-dept">
                <button
                  className="impact-dept-head"
                  onClick={() => setOpen(open === d.department ? null : d.department)}
                  aria-expanded={open === d.department}
                >
                  <span className="twisty">{open === d.department ? "▾" : "▸"}</span>
                  <span>{d.department}</span>
                  <span className="meta">
                    {summary(d.entities.map((e) => e.change))}
                  </span>
                </button>
                {open === d.department &&
                  d.entities.map((e) => (
                    <div key={e.id} className="impact-row" title={e.id}>
                      <span className={`change ${e.change}`}>{e.change}</span>
                      <span className="kind">{e.kind.split(".").slice(1).join(".") || e.kind}</span>
                      <span className="label">{e.name ?? "—"}</span>
                      <span className="meta">{e.attrs.join(", ")}</span>
                    </div>
                  ))}
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function summary(changes: string[]): string {
  const n = (k: string) => changes.filter((c) => c === k).length;
  return [
    n("created") && `${n("created")} created`,
    n("changed") && `${n("changed")} changed`,
    n("deleted") && `${n("deleted")} deleted`,
  ]
    .filter(Boolean)
    .join(" · ");
}

/** The shared undo history: every client's commits, in the order they landed. */
export function History({
  history,
  onUndo,
  onRedo,
}: {
  history: HistoryView | null;
  onUndo(): void;
  onRedo(): void;
}) {
  return (
    <div className="panel-body">
      <div className="history-actions">
        <button onClick={onUndo} disabled={!history?.undo_depth}>
          Undo
        </button>
        <button onClick={onRedo} disabled={!history?.redo_depth}>
          Redo
        </button>
        <span className="meta">
          {history ? `${history.undo_depth} undo · ${history.redo_depth} redo` : ""}
        </span>
      </div>
      {history?.redo.map((h, i) => (
        <div key={`r${i}`} className="history-row redo">
          <span className="label">{h.label}</span>
          <span className="meta">undone</span>
        </div>
      ))}
      {history &&
        [...history.undo].reverse().map((h, i) => (
          <div key={`u${h.first}-${i}`} className={`history-row${i === 0 ? " current" : ""}`}>
            <span className="label">{h.label}</span>
            <span className="meta">
              {h.cascade > 0 ? `+${h.cascade} reactions · ` : ""}rev {h.first}
            </span>
          </div>
        ))}
      {history && history.undo.length === 0 && history.redo.length === 0 && (
        <div className="empty">Nothing to undo.</div>
      )}
    </div>
  );
}
