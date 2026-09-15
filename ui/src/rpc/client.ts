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


/** An exact time. `ticks`/`rate` is the number; `seconds` is for layout only. */
export interface Time {
  ticks: number;
  rate_num: number;
  rate_den: number;
  seconds: number;
  timecode: string;
}

export interface Span {
  start: Time;
  duration: Time;
  end: Time;
}

export interface ProjectView {
  name: string;
  id: string | null;
  path: string;
  revision: number;
  nodes: number;
  edges: number;
  undo_depth: number;
  log_len: number;
  by_kind: Record<string, number>;
  clients: number;
  uptime_s: number;
}

/** The four states the whole project uses. Nothing else is renderable. */
export type CapabilityState = "verified" | "degraded" | "unavailable" | "refused";

export interface Capability {
  id: string;
  name: string;
  group: string;
  state: CapabilityState;
  detail: string;
}

export interface TimelineSummary {
  id: string;
  name: string;
  tracks: number;
}

export interface TimelineItem {
  index: number;
  id: string | null;
  kind: "clip" | "gap" | "transition";
  name: string;
  at: Span;
  source_range: Span | null;
  enabled: boolean;
  media: string | null;
  available_range: Span | null;
  handles: { head: Time; tail: Time } | null;
  link: string | null;
}

export interface TimelineTrack {
  index: number;
  id: string | null;
  name: string;
  kind: "video" | "audio";
  enabled: boolean;
  locked: boolean;
  muted: boolean;
  solo: boolean;
  targeted: boolean;
  duration: Time;
  items: TimelineItem[];
  markers: { name: string; at: Span; color: string | null }[];
}

export interface TimelineView {
  id: string;
  name: string;
  duration: Time;
  global_start_time: Time | null;
  tracks: TimelineTrack[];
  any_solo_video: boolean;
  any_solo_audio: boolean;
}

export interface MediaStream {
  id: string;
  name: string;
  codec: string | null;
  decodable: boolean | null;
}

export interface MediaView {
  id: string;
  name: string;
  path: string | null;
  online: boolean | null;
  hash: string | null;
  container: string | null;
  size: number | null;
  duration_s: number | null;
  video: {
    codec: string;
    width: number | null;
    height: number | null;
    frames: number | null;
    rate_num: number | null;
    rate_den: number | null;
    decodable: boolean;
  } | null;
  streams: MediaStream[];
}

export interface StructureView {
  scenes: {
    id: string;
    heading: string | null;
    number: string | null;
    eighths: number | null;
    est_duration_s: number | null;
    shots: { id: string; name: string; takes: { id: string; name: string; media: string[] }[] }[];
  }[];
}

export type TrackStateKey = "locked" | "muted" | "solo" | "targeted";

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
  project() { return this.call<ProjectView>("project"); }
  capabilities() { return this.call<{ capabilities: Capability[] }>("capabilities").then((r) => r.capabilities); }
  timelines() { return this.call<TimelineSummary[]>("timelines"); }
  timeline(id: string) { return this.call<TimelineView>("timeline", { id }); }
  trackState(track: string, key: TrackStateKey, value: boolean) {
    return this.call<CascadeResult>("track_state", { track, key, value });
  }
  media() { return this.call<MediaView[]>("media"); }
  ingest(path: string) { return this.call<{ id: string }>("ingest", { path }); }
  importOtio(text: string, key?: string) { return this.call<{ id: string }>("import_otio", { text, key }); }
  structure() { return this.call<StructureView>("structure"); }
  exportAs(what: string, scene?: number) { return this.call<string>("export", { what, scene }); }
}
