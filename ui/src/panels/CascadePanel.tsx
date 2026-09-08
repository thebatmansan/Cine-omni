import { CascadeResult, ChangeNotification } from "../rpc/client";

interface Props {
  cascade: CascadeResult | null;
  changes: ChangeNotification[];
}

/** Shows the reactor cascade of the last action and the live change feed. */
export function CascadePanel({ cascade, changes }: Props) {
  return (
    <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
      <div className="cascade">
        <div className="meta">Last action</div>
        {cascade ? (
          cascade.deltas.map((d) => (
            <div key={d.revision} className={`d${Math.min(d.depth, 3)}`}>
              {"  ".repeat(d.depth)}r{d.revision} {d.label} ({d.ops} ops)
            </div>
          ))
        ) : (
          <div className="meta">edit a dialogue line to see the cascade</div>
        )}
      </div>
      <div className="cascade">
        <div className="meta">Change feed</div>
        {changes.slice(0, 40).map((c) => (
          <div key={c.revision} className={`d${Math.min(c.depth, 3)}`}>
            r{c.revision} {c.label} · {c.touched.length} touched · {c.dirty} dirty
          </div>
        ))}
      </div>
    </div>
  );
}
