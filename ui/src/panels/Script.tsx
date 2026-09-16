import { useEffect, useRef, useState } from "react";
import type { ScriptScene, ScriptView, TimelineItem } from "../rpc/client";

/**
 * The screenplay, read from the graph, with dialogue editable in place.
 *
 * A line is edited in a local buffer while the writer types — that is an
 * unsent keystroke, not project state — and committed as one `edit_dialogue`
 * transaction on Enter or when focus leaves. Everything the change causes
 * (scene statistics, previs, logistics, flagged clips) arrives through the
 * impact panel and the refetch that follows the broadcast.
 */
export function Script({
  script,
  sceneId,
  onScene,
  onEditLine,
  selectedClip,
  onBind,
  onUnbind,
  onRevealClip,
  onIssueRevision,
}: {
  script: ScriptView | null;
  sceneId: string | null;
  onScene(id: string): void;
  onEditLine(line: string, text: string): void;
  selectedClip: TimelineItem | null;
  onBind(clip: string, scene: string): void;
  onUnbind(clip: string, scene: string): void;
  onRevealClip(clip: string): void;
  /** Issue revised pages: everything after them is marked against them. */
  onIssueRevision?(document: string): void;
}) {
  const doc = script?.documents[0] ?? null;
  const scene = doc?.scenes.find((s) => s.id === sceneId) ?? doc?.scenes[0] ?? null;

  return (
    <>
      <div className="panel scene-nav">
        <div className="panel-head">
          <span>Scenes</span>
          <span className="count">{doc?.scenes.length ?? 0}</span>
        </div>
        <div className="panel-body tree">
          {!doc && <div className="empty">No screenplay in this project. Import one from Project → Import screenplay.</div>}
          {doc?.scenes.map((s) => (
            <div
              key={s.id}
              className="tree-row"
              aria-selected={s.id === scene?.id}
              onMouseDown={() => onScene(s.id)}
              title={s.heading ?? ""}
            >
              <span className="kind">{s.number ?? "SC"}</span>
              <span className="label">{s.heading ?? "untitled"}</span>
              <span className="meta">
                {s.clips.some((c) => c.needs_conform) && <span className="conform-glyph glyph">!</span>}
                {s.eighths != null ? `${Math.floor(s.eighths / 8)} ${s.eighths % 8}/8` : ""}
              </span>
            </div>
          ))}
        </div>
      </div>

      <div className="panel script-page">
        <div className="panel-head">
          <span>Script</span>
          {doc && <span className="count">{doc.name || doc.source_key}</span>}
          <span className="spacer" />
          {doc && (
            <span className={`revision ${(doc.current_revision ?? "").split(" ")[0].toLowerCase()}`}>
              {doc.current_revision ?? "not issued"}
              {doc.marked > 0 && <span className="marked" title="elements changed since"> · {doc.marked}*</span>}
            </span>
          )}
          {doc && onIssueRevision && (
            <button
              className="issue"
              title={`Issue ${doc.next_colour} pages: everything after them is marked against them`}
              onClick={() => onIssueRevision(doc.id)}
            >
              Issue {doc.next_colour}
            </button>
          )}
          {scene && (
            <span className="count">
              {scene.words ?? 0} words · est {scene.est_duration_s?.toFixed(1) ?? "—"} s
            </span>
          )}
        </div>
        <div className="panel-body page-scroll">
          {scene && (
            <div className="page">
              <div className={`el heading${scene.revised ? " revised" : ""}`}>
                {scene.number && <span className="scene-number">{scene.number}</span>}
                {scene.heading}
                {scene.revised && (
                  <span className="mark" title={`${scene.revised} since the last revision`}>
                    *
                  </span>
                )}
              </div>
              {scene.elements.map((el) => (
                <Element key={el.id} el={el} onCommit={(text) => onEditLine(el.id, text)} />
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="panel scene-bindings">
        <div className="panel-head">
          <span>Cut material</span>
          <span className="spacer" />
          <span className="count">{scene?.clips.length ?? 0} bound</span>
        </div>
        <div className="panel-body pad">
          {scene && <Bindings scene={scene} selectedClip={selectedClip} onBind={onBind} onUnbind={onUnbind} onReveal={onRevealClip} />}
        </div>
      </div>
    </>
  );
}

function Bindings({
  scene,
  selectedClip,
  onBind,
  onUnbind,
  onReveal,
}: {
  scene: ScriptScene;
  selectedClip: TimelineItem | null;
  onBind(clip: string, scene: string): void;
  onUnbind(clip: string, scene: string): void;
  onReveal(clip: string): void;
}) {
  const canBind =
    selectedClip?.kind === "clip" && selectedClip.id && !scene.clips.some((c) => c.id === selectedClip.id);
  return (
    <>
      <p className="hint">
        A clip bound to a scene is told when the scene's words change. Nothing is bound by name or
        guess.
      </p>
      <button
        className="primary"
        disabled={!canBind}
        title={canBind ? "" : "Select a clip on the timeline that is not already bound"}
        onClick={() => selectedClip?.id && onBind(selectedClip.id, scene.id)}
      >
        Bind {selectedClip?.kind === "clip" ? `“${selectedClip.name}”` : "selected clip"}
      </button>
      {scene.clips.length === 0 && <div className="empty">No cut material bound to this scene.</div>}
      {scene.clips.map((c) => (
        <div key={c.id} className={`binding${c.needs_conform ? " conform" : ""}`}>
          <button className="link-button" onClick={() => onReveal(c.id)}>
            {c.name}
          </button>
          {c.needs_conform && <div className="reason">needs conform · {c.reason}</div>}
          <button onClick={() => onUnbind(c.id, scene.id)}>Unbind</button>
        </div>
      ))}
    </>
  );
}

function Element({
  el,
  onCommit,
}: {
  el: ScriptScene["elements"][number];
  onCommit(text: string): void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (draft != null) area.current?.focus();
  }, [draft != null]); // eslint-disable-line react-hooks/exhaustive-deps

  const editable = el.kind === "dialogue";
  const commit = () => {
    if (draft != null && draft !== (el.text ?? "")) onCommit(draft);
    setDraft(null);
  };

  // A revision mark sits in the margin, where a printed page has it.
  const mark = el.revised ? (
    <span className="mark" title={`${el.revised} since the last revision`}>
      *
    </span>
  ) : null;

  if (el.kind === "dialogue") {
    return (
      <div className={`el dialogue-block${el.revised ? " revised" : ""}`}>
        {mark}
        <div className="el character">{el.character}</div>
        {draft == null ? (
          <div
            className="el dialogue editable"
            title="Click to edit this line"
            onClick={() => editable && setDraft(el.text ?? "")}
          >
            {el.text}
          </div>
        ) : (
          <textarea
            ref={area}
            className="el dialogue editing"
            value={draft}
            rows={Math.max(1, Math.ceil(draft.length / 34))}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                commit();
              } else if (e.key === "Escape") {
                setDraft(null);
              }
            }}
          />
        )}
      </div>
    );
  }
  return (
    <div className={`el ${el.kind}${el.revised ? " revised" : ""}`}>
      {mark}
      {el.text}
    </div>
  );
}
