import { useEffect, useState } from "react";
import type { GradeView, RpcClient } from "../rpc/client";

/**
 * One clip's grade, as the runtime holds it.
 *
 * The runtime is asked rather than guessed at, and it answers with the whole
 * picture: the layers, the CDL beside them, which operations this build can set
 * and which it cannot. That last part is why this is a fetch and not a local
 * model — the interface offers exactly the controls the protocol carries, so a
 * control that would fail cannot appear.
 *
 * Refetched on every project revision: an undo, another window, or a reactor
 * changes a grade without anything here changing. `null` while nothing is
 * chosen or while the first answer is on its way; a refusal is returned as a
 * message rather than thrown away, because "this is not a clip" is an answer.
 */
export function useGrade(
  client: RpcClient,
  clip: string | null,
  revision: number,
): { grade: GradeView | null; refusal: string | null } {
  const [grade, setGrade] = useState<GradeView | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  useEffect(() => {
    if (!clip) {
      setGrade(null);
      setRefusal(null);
      return;
    }
    let live = true;
    client
      .grade(clip)
      .then((g) => {
        if (!live) return;
        setGrade(g);
        setRefusal(null);
      })
      .catch((e: Error) => {
        if (!live) return;
        setGrade(null);
        setRefusal(e.message);
      });
    return () => {
      live = false;
    };
  }, [client, clip, revision]);
  return { grade, refusal };
}
