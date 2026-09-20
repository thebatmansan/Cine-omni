import { useCallback, useEffect, useState } from "react";
import type { RpcClient } from "../rpc/client";

/**
 * What the user is working on, and which craft's view of it is open.
 *
 * Two axes, and only one of them persists. The **subject** is a node of the
 * project graph — a scene, a shot, a take, a media — and it survives
 * everything. The **lens** is which workspace is showing it, and it changes
 * freely. Choosing Grade while looking at Scene 17 leaves you on Scene 17,
 * which is true here by construction: the lens is never an input to what the
 * subject is.
 *
 * This is application state, not project state. Where a person is looking is
 * not a fact about the film, so it is not written into the graph — the graph
 * holds what the film *is*, and one editor's navigation is not that.
 */
export interface SubjectRef {
  id: string;
  kind: string;
}

export interface RelatedEntry {
  id: string;
  kind: string;
  kind_label: string;
  name: string;
  sub: string | null;
  group: string;
  /** The workspace that opens this kind of thing, where one does. */
  lens: string | null;
}

export interface SubjectView {
  subject: RelatedEntry;
  /** Outermost first; the subject itself is not in it. */
  path: RelatedEntry[];
  related: { group: string; entries: RelatedEntry[] }[];
}

export interface Subject {
  /** What is being worked on. */
  ref: SubjectRef | null;
  /** What the project connects to it, as the runtime answered. */
  view: SubjectView | null;
  /** True while the answer for a newly chosen subject is on its way. */
  loading: boolean;
  /** Why there is no view, when the subject could not be described. */
  refusal: string | null;
  select(ref: SubjectRef | null): void;
}

export function useSubject(client: RpcClient, revision: number | null): Subject {
  const [ref, setRef] = useState<SubjectRef | null>(null);
  const [view, setView] = useState<SubjectView | null>(null);
  const [loading, setLoading] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);

  const select = useCallback((next: SubjectRef | null) => {
    setRef(next);
    if (!next) {
      setView(null);
      setRefusal(null);
    }
  }, []);

  useEffect(() => {
    if (!ref) return;
    let live = true;
    setLoading(true);
    client
      .subject(ref.id)
      .then((v) => {
        if (!live) return;
        setView(v);
        setRefusal(null);
      })
      .catch((e: Error) => {
        if (!live) return;
        // A subject that has gone — undone, or deleted by another client —
        // is said, not silently replaced with something else.
        setView(null);
        setRefusal(e.message);
      })
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
    // The revision is in the dependencies so the relationships follow the
    // project: an edit that adds a shot to this scene shows up without the
    // user reselecting anything.
  }, [client, ref, revision]);

  return { ref, view, loading, refusal, select };
}
