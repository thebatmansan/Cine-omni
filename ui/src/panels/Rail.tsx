import type { RelatedEntry, SubjectView } from "../rpc/client";

/**
 * Where you are, what you are working on, and what the film connects to it.
 *
 * Not a tree of the project and not a file browser. It answers four questions
 * and nothing else: where am I, what am I on, what is connected to it, where
 * can I go next. Everything it shows is an edge somebody actually made —
 * there are no counts of things that do not exist, and an empty craft is
 * absent rather than filled in.
 *
 * It is deliberately quiet. The subject is the only thing with weight; the
 * path above it is small, the relationships below it are a dense list, and
 * the creative workspace beside it keeps its room.
 */
export function Rail({
  view,
  loading,
  refusal,
  lens,
  onSubject,
  onLens,
}: {
  view: SubjectView | null;
  loading: boolean;
  refusal: string | null;
  /** The workspace currently open, so the rail can mark which lens you are in. */
  lens: string;
  onSubject(entry: RelatedEntry): void;
  onLens(lens: string): void;
}) {
  if (refusal) {
    return (
      <div className="rail" aria-label="Context">
        <div className="rail-gone">
          {/* A subject that has gone is said. Quietly replacing it with
              something else would lose the user's place without telling them. */}
          This is no longer in the project — {refusal}
        </div>
      </div>
    );
  }
  if (!view) {
    return (
      <div className="rail" aria-label="Context">
        <div className="rail-empty">
          Choose a scene, a shot or a take to work on.
        </div>
      </div>
    );
  }
  const { subject, path, related } = view;
  return (
    <div className={`rail${loading ? " loading" : ""}`} aria-label="Context">
      {path.length > 0 && (
        <div className="rail-path">
          {path.map((p, i) => (
            <span key={p.id}>
              {i > 0 && <span className="rail-sep">/</span>}
              <button className="rail-crumb" onClick={() => onSubject(p)} title={p.kind_label}>
                {p.name}
              </button>
            </span>
          ))}
        </div>
      )}

      <div className="rail-subject">
        <div className="rail-kind">{subject.kind_label}</div>
        <div className="rail-name">{subject.name}</div>
        {subject.sub && <div className="rail-sub">{subject.sub}</div>}
      </div>

      {subject.lens && subject.lens !== lens && (
        <button className="rail-open" onClick={() => onLens(subject.lens!)}>
          Open in {subject.lens}
        </button>
      )}

      {related.length === 0 && (
        <div className="rail-empty">Nothing is connected to this yet.</div>
      )}
      {related.map((group) => (
        <div className="rail-group" key={group.group}>
          <div className="rail-group-name">{group.group}</div>
          {group.entries.map((e) => (
            <button
              className="rail-entry"
              key={e.id}
              onClick={() => onSubject(e)}
              title={e.sub ?? e.kind}
            >
              <span className="rail-entry-kind">{e.kind_label}</span>
              <span className="rail-entry-name">{e.name}</span>
              {/* Where a related thing belongs to another craft, say which —
                  that is the whole point of one subject and many lenses. */}
              {e.lens && e.lens !== lens && <span className="rail-entry-lens">{e.lens}</span>}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
