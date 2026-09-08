// JSON-RPC 2.0 client over WebSocket (via server/bridge.mjs → `cine serve`).
// Also receives `changed` notifications pushed by the engine after each commit.

export interface ChangeNotification {
  revision: number;
  label: string;
  depth: number;
  touched: string[];
  dirty: number;
}

export interface CascadeResult {
  label: string;
  deltas: { revision: number; label: string; depth: number; ops: number }[];
}

export interface SceneSummary {
  id: string;
  heading: string | null;
  number: string | null;
  words: number | null;
  eighths: number | null;
  est_duration_s: number | null;
  lines: string[];
}

export interface NodeView {
  id: string;
  kind: string;
  attrs: Record<string, unknown>;
  children: string[];
  edges_out: { from: string; to: string; kind: string }[];
  edges_in: { from: string; to: string; kind: string }[];
}

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void };

export class RpcClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private listeners = new Set<(n: ChangeNotification) => void>();
  private stateListeners = new Set<(open: boolean) => void>();

  constructor(private url: string) {}

  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      ws.onopen = () => {
        this.ws = ws;
        this.stateListeners.forEach((l) => l(true));
        resolve();
      };
      ws.onerror = () => reject(new Error(`cannot connect to ${this.url}`));
      ws.onclose = () => {
        this.ws = null;
        this.stateListeners.forEach((l) => l(false));
        for (const p of this.pending.values()) p.reject(new Error("connection closed"));
        this.pending.clear();
      };
      ws.onmessage = (ev) => this.onMessage(String(ev.data));
    });
  }

  /** Feed a raw JSON-RPC line (exposed for tests). */
  onMessage(line: string): void {
    let msg: any;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (msg.method === "changed") {
      this.listeners.forEach((l) => l(msg.params as ChangeNotification));
      return;
    }
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.error) p.reject(new Error(msg.error.message));
    else p.resolve(msg.result);
  }

  onChange(l: (n: ChangeNotification) => void): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  onState(l: (open: boolean) => void): () => void {
    this.stateListeners.add(l);
    return () => this.stateListeners.delete(l);
  }

  /** Build the wire message (exposed for tests). */
  encode(method: string, params?: unknown): { id: number; line: string } {
    const id = this.nextId++;
    return { id, line: JSON.stringify({ jsonrpc: "2.0", id, method, params: params ?? null }) };
  }

  call<T = unknown>(method: string, params?: unknown): Promise<T> {
    const { id, line } = this.encode(method, params);
    return new Promise<T>((resolve, reject) => {
      if (!this.ws) return reject(new Error("not connected"));
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.ws.send(line);
    });
  }

  // Typed helpers matching cine-cli's `serve` methods.
  stats() { return this.call<Record<string, unknown>>("stats"); }
  scenes() { return this.call<SceneSummary[]>("scenes"); }
  node(id: string) { return this.call<NodeView>("node", { id }); }
  ofKind(kind: string) { return this.call<{ id: string; name: string }[]>("kind", { kind }); }
  sceneReport(scene: string) { return this.call<string>("scene_report", { scene }); }
  editDialogue(line: string, text: string) { return this.call<CascadeResult>("edit_dialogue", { line, text }); }
  set(id: string, key: string, value: unknown) { return this.call<CascadeResult>("set", { id, key, value }); }
  undo() { return this.call<CascadeResult>("undo"); }
  redo() { return this.call<CascadeResult>("redo"); }
  save() { return this.call<boolean>("save"); }
  exportAs(what: string, scene?: number) { return this.call<string>("export", { what, scene }); }
}
