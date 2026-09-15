import { useEffect, useState } from "react";
import type { RpcClient } from "../rpc/client";

type Orphan = { scene: string; heading: string; clips: number; takes: number };

/**
 * The whole screenplay as Fountain, rewritten in place.
 *
 * The text is a draft until Apply: then it is one rewrite of the document
 * through the runtime, which keeps every scene whose identity survives —
 * numbered scenes keep their number whatever their heading — and refuses to
 * remove a scene that holds bound clips or recorded takes until the writer
 * confirms, having been told exactly which.
 */
export function Writer({
  client,
  document,
  revision,
  run,
}: {
  client: RpcClient;
  document: string;
  revision: number;
  run<T>(label: string, call: () => Promise<T>): Promise<T | null>;
}) {
  const [base, setBase] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [orphans, setOrphans] = useState<Orphan[] | null>(null);
  const dirty = base !== null && draft !== base;

  // Load, and reload when the project moves on — unless the writer has
  // unsaved changes, which are never overwritten.
  useEffect(() => {
    if (dirty) return;
    void client.scriptText(document).then((t) => {
      setBase(t);
      setDraft(t);
    });
  }, [client, document, revision]); // eslint-disable-line react-hooks/exhaustive-deps

  const apply = async (allow: boolean) => {
    const r = await run("rewrite screenplay", () => client.scriptWrite(document, draft, allow));
    if (!r) return;
    if (r.refused) {
      setOrphans(r.orphans);
      return;
    }
    setOrphans(null);
    setBase(draft);
  };

  return (
    <div className="panel writer">
      <div className="panel-head">
        <span>Write</span>
        <span className="count">Fountain</span>
        <span className="spacer" />
        {dirty && <span className="count">unapplied changes</span>}
        <button disabled={!dirty} onClick={() => base !== null && setDraft(base)}>
          Discard
        </button>
        <button className="primary" disabled={!dirty} onClick={() => void apply(false)}>
          Apply
        </button>
      </div>
      {orphans && (
        <div className="notice bad">
          <strong>This rewrite removes scenes that hold production work</strong>
          {orphans.map((o) => (
            <div key={o.scene}>
              {o.heading || "(untitled)"} — {o.clips} bound clip{o.clips === 1 ? "" : "s"}, {o.takes} take
              {o.takes === 1 ? "" : "s"}
            </div>
          ))}
          <div className="hint">
            If you only renamed a scene, give it a scene number (e.g. <code>#12#</code>) before renaming: a numbered
            scene keeps its identity.
          </div>
          <div className="binding">
            <button onClick={() => setOrphans(null)}>Keep editing</button>
            <button className="danger" onClick={() => void apply(true)}>
              Remove them anyway
            </button>
          </div>
        </div>
      )}
      <textarea
        className="fountain"
        value={draft}
        spellCheck={false}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            if (dirty) void apply(false);
          }
        }}
      />
    </div>
  );
}
