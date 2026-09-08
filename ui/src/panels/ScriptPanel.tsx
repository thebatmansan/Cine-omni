import { useEffect, useState } from "react";
import { NodeView, RpcClient, SceneSummary } from "../rpc/client";

interface Props {
  client: RpcClient;
  scenes: SceneSummary[];
  selected: string | null;
  onSelect: (id: string) => void;
  onEdit: (line: string, text: string) => Promise<void>;
}

/** Scene list plus an editor for the selected scene's dialogue lines. */
export function ScriptPanel({ client, scenes, selected, onSelect, onEdit }: Props) {
  const scene = scenes.find((s) => s.id === selected) ?? null;
  const [lines, setLines] = useState<NodeView[]>([]);
  const [drafts, setDrafts] = useState<Record<string, string>>({});

  useEffect(() => {
    if (!scene) return;
    Promise.all(scene.lines.map((id) => client.node(id))).then((ns) => {
      setLines(ns);
      setDrafts(Object.fromEntries(ns.map((n) => [n.id, String(n.attrs.text ?? "")])));
    });
  }, [scene, client]);

  return (
    <div>
      {scenes.map((s) => (
        <div key={s.id} className={`scene ${s.id === selected ? "selected" : ""}`} onClick={() => onSelect(s.id)}>
          <div>{s.number ? `${s.number}. ` : ""}{s.heading}</div>
          <div className="meta">{s.words ?? 0} words · {s.eighths ?? 0}/8 pages · {(s.est_duration_s ?? 0).toFixed(1)} s</div>
        </div>
      ))}
      {scene && (
        <div style={{ marginTop: 12 }}>
          {lines.map((l) => (
            <div key={l.id} style={{ marginBottom: 8 }}>
              <div className="meta">{String(l.attrs.character ?? "")}</div>
              <textarea
                rows={2}
                value={drafts[l.id] ?? ""}
                onChange={(e) => setDrafts({ ...drafts, [l.id]: e.target.value })}
                onBlur={() => {
                  if (drafts[l.id] !== String(l.attrs.text ?? "")) onEdit(l.id, drafts[l.id]);
                }}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
