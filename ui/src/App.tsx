import { useCallback, useEffect, useMemo, useState } from "react";
import { CascadeResult, ChangeNotification, RpcClient, SceneSummary } from "./rpc/client";
import { ScriptPanel } from "./panels/ScriptPanel";
import { Viewport } from "./panels/Viewport";
import { CascadePanel } from "./panels/CascadePanel";
import { InspectorPanel } from "./panels/InspectorPanel";

// The runtime serves this page and its API from the same origin, so the socket
// address is derived rather than configured — the runtime may have moved to
// another port if 7331 was busy, and a hard-coded default would then connect to
// nothing (or, worse, to a different project's runtime).
//
// VITE_CINE_BRIDGE overrides it for `vite dev`, where the page is served by
// vite on 5173 and the runtime is elsewhere.
function runtimeUrl(): string {
  const override = (import.meta as any).env?.VITE_CINE_BRIDGE;
  if (override) return override;
  if (typeof window !== "undefined" && window.location?.host) {
    const scheme = window.location.protocol === "https:" ? "wss" : "ws";
    return `${scheme}://${window.location.host}/ws`;
  }
  return "ws://127.0.0.1:7331/ws";
}

const BRIDGE_URL = runtimeUrl();

export function App() {
  const client = useMemo(() => new RpcClient(BRIDGE_URL), []);
  const [connected, setConnected] = useState(false);
  const [scenes, setScenes] = useState<SceneSummary[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [report, setReport] = useState("");
  const [cascade, setCascade] = useState<CascadeResult | null>(null);
  const [changes, setChanges] = useState<ChangeNotification[]>([]);
  const [revision, setRevision] = useState<number>(0);

  const refresh = useCallback(async () => {
    const s = await client.scenes();
    setScenes(s);
    const stats = await client.stats();
    setRevision(Number(stats.revision ?? 0));
    if (selected) setReport(await client.sceneReport(selected));
  }, [client, selected]);

  useEffect(() => {
    client.onState(setConnected);
    client.onChange((n) => setChanges((c) => [n, ...c].slice(0, 200)));
    client.connect().then(refresh).catch(() => setConnected(false));
  }, [client]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (selected && connected) client.sceneReport(selected).then(setReport);
  }, [selected, connected, revision, client]);

  const onEdit = async (line: string, text: string) => {
    const r = await client.editDialogue(line, text);
    setCascade(r);
    await refresh();
  };
  const onUndo = async () => { setCascade(await client.undo()); await refresh(); };
  const onRedo = async () => { setCascade(await client.redo()); await refresh(); };

  return (
    <div className="app">
      <div className="topbar">
        <strong>CINE-OMNI</strong>
        <button onClick={onUndo}>Undo</button>
        <button onClick={onRedo}>Redo</button>
        <button onClick={() => client.save()}>Save</button>
        <span className="status">{connected ? `connected · revision ${revision}` : `not connected to ${BRIDGE_URL}`}</span>
      </div>
      <div className="panel">
        <h2>Screenplay</h2>
        <div className="body">
          <ScriptPanel client={client} scenes={scenes} selected={selected} onSelect={setSelected} onEdit={onEdit} />
        </div>
      </div>
      <div className="panel">
        <h2>Previs viewport</h2>
        <Viewport client={client} scene={selected} revision={revision} />
      </div>
      <div className="panel">
        <h2>Inspector</h2>
        <div className="body">
          <InspectorPanel report={report} />
        </div>
      </div>
      <div className="panel bottom">
        <h2>Context graph activity</h2>
        <div className="body">
          <CascadePanel cascade={cascade} changes={changes} />
        </div>
      </div>
    </div>
  );
}
