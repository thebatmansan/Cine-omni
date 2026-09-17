import { useEffect, useState } from "react";
import type { CaptionsView, RpcClient } from "../rpc/client";

/**
 * A sequence's captions for one language, as the runtime projects them.
 *
 * One fetch shared by everything that shows captions — the Captions panel and
 * the caption lane under the timeline — so the two cannot disagree about what
 * a caption says or whether it needs review. Refetched on every project
 * revision, because another window's edit or a script rewrite changes captions
 * without anything here changing.
 *
 * When no language is chosen yet, the runtime picks the first the sequence
 * has, and `onLanguage` reports it so the choice is held in one place.
 */
export function useCaptions(
  client: RpcClient,
  timeline: string | null,
  language: string | null,
  revision: number,
  onLanguage: (l: string) => void,
): CaptionsView | null {
  const [view, setView] = useState<CaptionsView | null>(null);
  useEffect(() => {
    if (!timeline) {
      setView(null);
      return;
    }
    let live = true;
    client
      .captions(timeline, language ?? undefined)
      .then((v) => {
        if (!live) return;
        setView(v);
        if (!language && v.language) onLanguage(v.language);
      })
      .catch(() => live && setView(null));
    return () => {
      live = false;
    };
  }, [client, timeline, language, revision]); // eslint-disable-line react-hooks/exhaustive-deps
  return view;
}
