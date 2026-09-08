/** Everything derived from the selected scene, straight from `scene_report`. */
export function InspectorPanel({ report }: { report: string }) {
  if (!report) return <div className="meta">select a scene</div>;
  return (
    <pre>
      {report.split("\n").map((line, i) => (
        <div key={i} className={/stale=true|needs_conform=true/.test(line) ? "stale" : ""}>{line}</div>
      ))}
    </pre>
  );
}
