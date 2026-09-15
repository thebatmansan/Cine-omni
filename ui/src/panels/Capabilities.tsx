import { Capability } from "../rpc/client";

/**
 * What this build can do, in the words of the systems that know.
 *
 * Rendered straight from the `capabilities` method. Nothing here is written in
 * the browser: if a capability is missing from this list it is because the
 * runtime did not report it, which is the correct failure — an interface that
 * invented an entry would be inventing a claim.
 */
export function Capabilities({ capabilities }: { capabilities: Capability[] }) {
  const groups = new Map<string, Capability[]>();
  for (const c of capabilities) {
    const list = groups.get(c.group) ?? [];
    list.push(c);
    groups.set(c.group, list);
  }
  const counts = capabilities.reduce<Record<string, number>>((acc, c) => {
    acc[c.state] = (acc[c.state] ?? 0) + 1;
    return acc;
  }, {});

  return (
    <div className="panel">
      <div className="panel-head">
        <span>Capabilities</span>
        <span className="count">
          {counts.verified ?? 0} verified · {counts.degraded ?? 0} degraded ·{" "}
          {(counts.unavailable ?? 0) + (counts.refused ?? 0)} not available
        </span>
      </div>
      <div className="panel-body caps">
        {[...groups.entries()].map(([group, items]) => (
          <div key={group}>
            <div className="cap-group">{group}</div>
            {items.map((c) => (
              <div className="cap" key={c.id} title={c.detail}>
                <span className={`badge dot ${c.state}`} />
                <span className="cap-name">{c.name}</span>
                <span className="cap-detail">{c.detail}</span>
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
