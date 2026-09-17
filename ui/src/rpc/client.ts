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
  /** Present on every method that changes the project. */
  impact?: ImpactView;
}

/** An exact time as the wire carries it. Extra display fields are ignored. */
export interface ExactTime {
  ticks: number;
  rate_num: number;
  rate_den: number;
}

export interface ExactRange {
  start_time: ExactTime;
  duration: ExactTime;
}

export type Edge = "head" | "tail";

/** New material: a range of an asset the project already holds. */
export interface ClipSpec {
  media: string;
  source_range: ExactRange;
  name?: string;
}

/** An in point, an out point, or one of each. `null` means unmarked. */
export interface EditMarks {
  in: ExactTime | null;
  out: ExactTime | null;
}

/**
 * What to do when all four marks are set and the two durations disagree.
 *
 * Fit-to-fill is deliberately absent: it changes the clip's speed, and the
 * runtime has no time effect to carry the speed, so it says so rather than
 * offering a button that does a trim under another name.
 */
export type MarkMismatch = "refuse" | "trim_tail" | "trim_head" | "honour_source" | "fit_to_fill";

/**
 * A clip's speed, as the runtime worked it out.
 *
 * `media`/`track` is the number — an exact ratio of two integers. `label` is
 * for a badge and nothing computes from it.
 */
export interface ClipSpeed {
  media: number;
  track: number;
  reversed: boolean;
  label: string;
}

/** A resolved three- or four-point edit, as the runtime worked it out. */
export interface EditPlan {
  source: Span;
  record: Span;
  /** The record in was derived from the record out: the edit ends on its mark. */
  backtimed: boolean;
  /** Which points were arithmetic rather than marked. */
  derived: string[];
  /** How a four-point disagreement was settled, if there was one. */
  reconciled: string | null;
  /** The speed change the marks asked for, when they asked for one. */
  retime: { played: Time; reversed: boolean } | null;
  description: string;
}

/** One thing a delivery would reject about a caption, in an editor's words. */
export interface CaptionIssue {
  caption: number;
  kind:
    | "too_many_lines"
    | "line_too_long"
    | "too_short"
    | "too_long"
    | "gap_too_small"
    | "overlap"
    | "reading_speed"
    | "empty";
  message: string;
}

/** The line of dialogue a caption transcribes, as the runtime resolved it. */
export interface CaptionLink {
  line: string | null;
  scene: string;
  heading: string | null;
  character: string;
  /** The words as transcribed. */
  transcribed: string;
  /** The words the line says now, when it still exists. */
  now: string | null;
}

export interface CaptionCandidate {
  line: { id: string; scene: string; character: string; ordinal: number; text: string };
  heading: string;
  score: number;
}

export interface CaptionView {
  index: number;
  key: string | null;
  link: CaptionLink | null;
  /** Why this caption needs review against the script or the cut; null if it does not. */
  review: string | null;
  record: Span;
  lines: string[];
  placement: "bottom" | "top";
  speaker: string | null;
  issues: CaptionIssue[];
}

export interface CaptionTrackView {
  language: string;
  rate_num: number;
  rate_den: number;
  captions: CaptionView[];
  issue_count: number;
}

export interface CaptionsView {
  languages: string[];
  language: string | null;
  track: CaptionTrackView | null;
}

export type CaptionFormat = "imsc" | "srt" | "vtt" | "scc";

/** One angle of a multicam clip at an instant, as the runtime resolved it. */
export interface AngleAt {
  key: string;
  active: boolean;
  asset: string | null;
  name: string;
  /** Where this angle's media sits against the take (set by syncing). */
  offset: Time;
  media: Time | null;
  /** The picture of the file this angle shows, when it has one here. */
  index: number | null;
  why_not: string | null;
}

export interface AnglesAt {
  clip: string | null;
  angles: AngleAt[];
}

/** An edit, exactly as `cine_timeline::EditOp` deserializes it. */
export type EditOp =
  | { op: "ripple_trim"; index: number; edge: Edge; delta: ExactTime }
  | { op: "roll"; cut: number; delta: ExactTime }
  | { op: "slip"; index: number; delta: ExactTime }
  | { op: "slide"; index: number; delta: ExactTime }
  | { op: "split_at"; time: ExactTime }
  | { op: "extract"; range: ExactRange }
  | { op: "lift"; range: ExactRange }
  | { op: "insert"; time: ExactTime; clip: ClipSpec }
  | { op: "overwrite"; time: ExactTime; clip: ClipSpec }
  | { op: "replace"; index: number; clip: ClipSpec }
  | { op: "join"; index: number };

export interface EditResult {
  outcome: {
    label: string;
    requested: Time | null;
    applied: Time | null;
    clamped: boolean;
    item: string | null;
    changed_tracks: string[];
    linked_clips: number;
  };
  cascade: CascadeResult;
  impact: ImpactView;
}

export interface ImpactEntity {
  id: string;
  kind: string;
  name: string | null;
  change: "created" | "deleted" | "changed";
  attrs: string[];
  edges: number;
  reason: string | null;
}

export interface ImpactView {
  label: string;
  revisions: number[];
  steps: {
    revision: number;
    depth: number;
    cause: number | null;
    by: string;
    action: string;
    ops: number;
    actor: string;
  }[];
  departments: { department: string; count: number; entities: ImpactEntity[] }[];
  conform: { clip: string; name: string | null; reason: string | null }[];
  entities: number;
}

export interface HistoryEntry {
  label: string;
  first: number | null;
  last: number | null;
  deltas: number;
  cascade: number;
  actor: string | null;
}

export interface HistoryView {
  undo: HistoryEntry[];
  redo: HistoryEntry[];
  undo_depth: number;
  redo_depth: number;
}

export interface ScriptElement {
  id: string;
  kind: string;
  text: string | null;
  character: string | null;
  /** `"changed"` or `"added"` since the last revision went out; null if neither. */
  revised: "changed" | "added" | null;
  /** Captions transcribing this line; `review` says why one needs looking at. */
  captions?: { id: string; review: string | null }[];
}

export interface ScriptScene {
  id: string;
  heading: string | null;
  number: string | null;
  words: number | null;
  eighths: number | null;
  est_duration_s: number | null;
  characters: string[] | null;
  revised: "changed" | "added" | null;
  elements: ScriptElement[];
  clips: {
    id: string;
    name: string;
    timeline: string | null;
    track: string | null;
    needs_conform: boolean;
    reason: string | null;
  }[];
}

export interface ScriptView {
  documents: ScriptDocument[];
}

export interface ScriptDocument {
  id: string;
  name: string;
  source_key: string | null;
  scenes: ScriptScene[];
  /** Pages that have gone out, oldest first. */
  revisions: { colour: string; round: number; date: string; note: string; label: string }[];
  /** What the crew is holding, e.g. `"Blue — 2026-01-19"`. */
  current_revision: string | null;
  /** What the next set of pages would be called. */
  next_colour: string;
  /** How many elements carry a revision mark. */
  marked: number;
  /** Elements that were in the last revision and have since been cut. */
  removed_since_revision: string[];
}

export interface LDate { year: number; month: number; day: number }
export interface LTime { hour: number; minute: number }

export interface Strip {
  scene: string;
  number: string;
  pages_eighths: number;
  int_ext: string;
  location: string;
  time_of_day: string;
  cast: string[];
  est_minutes: number;
}

export interface ShootDay {
  index: number;
  date: LDate;
  strips: Strip[];
  total_eighths: number;
  cast_on_call: string[];
  locations: string[];
  is_night: boolean;
}

export interface SceneBreakdown {
  scene: string;
  number: string;
  heading: string;
  int_ext: string;
  location: string;
  time_of_day: string;
  cast: string[];
  props: string[];
  extras_hint: string | null;
  page_eighths: number;
  est_minutes: number;
  vehicles: boolean;
  animals: boolean;
  sfx: boolean;
  vfx: boolean;
  /** What a person tagged by hand; the lists above are the merged result. */
  tags: SceneTags;
}

/** A person's corrections to what the script implies. Survives a rewrite. */
export interface SceneTags {
  props_added: string[];
  props_removed: string[];
  cast_added: string[];
  cast_removed: string[];
  /** `null` leaves the keyword search to decide; a boolean overrules it. */
  vehicles: boolean | null;
  animals: boolean | null;
  sfx: boolean | null;
  vfx: boolean | null;
}

export interface CallSheetView {
  day: number;
  date: LDate;
  general_call: LTime;
  scenes: { scene: string; number: string; heading: string; pages: string; cast_numbers: number[]; location: string }[];
  cast_calls: { name: string; number: number; pickup: LTime; makeup: LTime; on_set: LTime }[];
  crew_calls: { department: string; call: LTime }[];
  locations: string[];
  weather_placeholder: string;
  notes: string[];
}

export interface ScheduleConstraints {
  max_pages_per_day: number;
  max_hours_per_day: number;
  shooting_days_per_week: number;
  start_date: LDate;
  cast_availability: Record<string, unknown>;
  group_by_location: boolean;
  night_shoots_together: boolean;
  /** Scenes a person put on a day: scene id → shooting day number. */
  pinned?: Record<string, number>;
}

/** The project's colour config, and what every file in it is in. */
export interface ColorView {
  config: { name: string; source: string | null; unimplemented: string[] } | null;
  /** What everything is being looked at through; null means the config's own default. */
  view: ViewSelection | null;
  displays: { name: string; active: boolean; views: { name: string; active: boolean }[] }[];
  colorspaces: {
    name: string;
    family: string;
    encoding: string;
    is_display: boolean;
    isdata: boolean;
  }[];
  media: ColorMediaView[];
  /** The clips of the open sequence, with how each one is graded. */
  clips: ColorClipView[];
}

/** One clip of the sequence, as the colour workspace sees it. */
export interface ColorClipView {
  clip: string;
  name: string;
  input_colorspace: string | null;
  graded: boolean;
  cdl: Cdl | null;
}

/** ASC CDL: slope, offset and power per channel, then saturation. */
export interface Cdl {
  slope: [number, number, number];
  offset: [number, number, number];
  power: [number, number, number];
  saturation: number;
}

export interface ViewSelection {
  display: string;
  view: string;
}

export interface ColorMediaView {
  id: string;
  name: string;
  path: string | null;
  /** Null when nothing has established what this file is in. */
  input_colorspace: string | null;
  /** `rule:<file rule>` when the config decided, `user` when a person did. */
  source: string | null;
}

/** Day Out of Days: what the stripboard means for the cast. */
export interface DayOutOfDaysView {
  /** Day numbers, in order; a cast row's entries line up with these. */
  days: number[];
  cast: {
    name: string;
    /** `SW`, `W`, `H`, `WF`, `SWF`, or empty for a day they are not engaged. */
    days: string[];
    start: number;
    finish: number;
    worked: number;
    /** Days held between first and last: not worked, not released, still paid. */
    held: number;
  }[];
  total_worked: number;
  total_held: number;
}

export interface ProductionView {
  document: string;
  breakdowns: SceneBreakdown[];
  schedule: { days: ShootDay[]; warnings: string[]; company_moves: number } | null;
  dood: DayOutOfDaysView | null;
  call_sheets: CallSheetView[];
  constraints: ScheduleConstraints;
  shots: {
    scene: string;
    shots: {
      id: string;
      number: string;
      size: string;
      movement: string;
      lens_mm: number;
      camera_body: string;
      support: string;
      description: string;
      stale: boolean;
      takes: { id: string; name: string }[];
    }[];
  }[];
}

export type JobState = "queued" | "running" | "succeeded" | "failed" | "cancelled";

export interface JobView {
  id: number;
  kind: string;
  label: string;
  state: JobState;
  progress: number | null;
  message: string | null;
  attempts: number;
  max_attempts: number;
  error: string | null;
  output: Record<string, unknown> | null;
  resource: "cpu" | "gpu" | "io";
  priority: number;
  resumed: boolean;
  elapsed_ms: number | null;
  params: Record<string, unknown>;
}

export interface ChecksumRecord {
  algorithm: string;
  sha256: string;
  size: number;
  last?: { result: "established" | "match" | "mismatch"; sha256: string; seconds: number; job: number };
}

export interface PictureAt {
  black: boolean;
  track?: string | null;
  track_name?: string;
  clip?: string | null;
  name?: string;
  asset?: string | null;
  online?: boolean | null;
  source?: Time;
  index?: number | null;
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
  /** `null` when the clip plays in real time, which is nearly always. */
  speed: ClipSpeed | null;
  /** A retimed clip's sound keeps its pitch (time stretch) rather than varispeed. */
  keep_pitch: boolean;
  /** A speed ramp's segments, each end's speed an exact `[num, den]`; null if constant. */
  ramp: { length: Time; from: [number, number]; to: [number, number] }[] | null;
  /** Every angle this clip could play; `null` when it carries one reference. */
  angles: string[] | null;
  /** Which of them is playing. */
  angle: string | null;
  needs_conform: boolean;
  conform_reason: string | null;
  /** Scenes (or shots) this clip is bound to. */
  scenes: string[];
  /** The asset entity the clip plays. */
  asset: string | null;
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
  checksum: ChecksumRecord | null;
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
  private focusListeners = new Set<(timeline: string | null) => void>();
  private viewListeners = new Set<(view: ViewSelection | null) => void>();
  private jobListeners = new Set<(job: JobView) => void>();

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
    if (msg.method === "job") {
      this.jobListeners.forEach((l) => l(msg.params as JobView));
      return;
    }
    if (msg.method === "focus") {
      this.focusListeners.forEach((l) => l((msg.params?.timeline as string | null) ?? null));
      return;
    }
    if (msg.method === "view") {
      this.viewListeners.forEach((l) => l((msg.params as ViewSelection | null) ?? null));
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

  /** A job changed state or reported progress. */
  onJob(l: (job: JobView) => void): () => void {
    this.jobListeners.add(l);
    return () => this.jobListeners.delete(l);
  }

  /** Another surface chose what everything is looked at through. */
  onView(l: (view: ViewSelection | null) => void): () => void {
    this.viewListeners.add(l);
    return () => this.viewListeners.delete(l);
  }

  /** Another surface opened a sequence. */
  onFocus(l: (timeline: string | null) => void): () => void {
    this.focusListeners.add(l);
    return () => this.focusListeners.delete(l);
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
  edit(track: string, op: EditOp, linked = true) {
    return this.call<EditResult>("edit", { track, op, linked });
  }
  place(tracks: string[], time: ExactTime, clip: ClipSpec, mode: "insert" | "overwrite") {
    return this.call<CascadeResult & { clips: string[] }>("place", { tracks, time, clip, mode });
  }
  threePoint(
    tracks: string[],
    media: string,
    source: EditMarks,
    record: EditMarks,
    mode: "insert" | "overwrite",
    mismatch: MarkMismatch = "refuse",
  ) {
    return this.call<CascadeResult & { clips: string[]; plan: EditPlan }>("three_point", {
      tracks,
      media,
      source,
      record,
      mode,
      mismatch,
    });
  }
  captions(timeline: string, language?: string) {
    return this.call<CaptionsView>("captions", language ? { timeline, language } : { timeline });
  }
  setCaption(
    timeline: string,
    language: string,
    caption: { index?: number; start: ExactTime; end: ExactTime; text: string; placement?: string; speaker?: string },
  ) {
    return this.call<CascadeResult & { track: CaptionTrackView }>("set_caption", { timeline, language, ...caption });
  }
  removeCaption(timeline: string, language: string, index: number) {
    return this.call<CascadeResult & { track: CaptionTrackView }>("remove_caption", { timeline, language, index });
  }
  importCaptions(timeline: string, language: string, text: string) {
    return this.call<
      CascadeResult & {
        track: CaptionTrackView;
        report: { captions: number; snapped: number; largest_snap_ms: number; skipped: string[] };
      }
    >("import_captions", { timeline, language, text });
  }
  captionCandidates(timeline: string, language: string, key: string) {
    return this.call<{ scenes: string[]; candidates: CaptionCandidate[] }>("caption_candidates", {
      timeline,
      language,
      key,
    });
  }
  linkCaption(timeline: string, language: string, key: string, line: string) {
    return this.call<CascadeResult & { track: CaptionTrackView }>("link_caption", { timeline, language, key, line });
  }
  unlinkCaption(timeline: string, language: string, key: string) {
    return this.call<CascadeResult & { track: CaptionTrackView }>("unlink_caption", { timeline, language, key });
  }
  acceptCaptionLine(timeline: string, language: string, key: string) {
    return this.call<CascadeResult & { track: CaptionTrackView }>("accept_caption_line", { timeline, language, key });
  }
  linkCaptionsExact(timeline: string, language: string) {
    return this.call<CascadeResult & { linked: number; track?: CaptionTrackView }>("link_captions_exact", {
      timeline,
      language,
    });
  }
  exportCaptions(timeline: string, language: string, format: CaptionFormat) {
    return this.call<{ text: string; mime: string; extension: string; largest_rounding_us?: number }>(
      "export_captions",
      { timeline, language, format },
    );
  }
  /** Every angle of the multicam clip on screen at a time: which picture each shows, or why none. */
  anglesAt(timeline: string, time: ExactTime) {
    return this.call<AnglesAt>("angles_at", { timeline, time });
  }
  /** Give a clip more angles: assets it could play instead of the one it plays. */
  addAngles(clip: string, angles: { key: string; media: string }[]) {
    return this.call<CascadeResult>("add_angles", { clip, angles });
  }
  /** Play a different angle on one clip, without cutting. */
  setAngle(clip: string, key: string) {
    return this.call<CascadeResult>("set_angle", { clip, key });
  }
  /** Cut to another angle at a point on the track: the multicam switch. */
  switchAngle(track: string, at: ExactTime, key: string) {
    return this.call<CascadeResult & { clip: string }>("switch_angle", { track, at, key });
  }
  /** Set a clip's speed by saying how long it should play; `null` removes it. */
  retime(
    clip: string,
    played: ExactTime | null,
    reversed = false,
    keepPitch = false,
    ramp: { length: ExactTime; from: string; to: string }[] | null = null,
  ) {
    return this.call<CascadeResult>("retime", { clip, played, reversed, keep_pitch: keepPitch, ramp });
  }
  newTimeline(name: string, rate_num: number, rate_den: number) {
    return this.call<CascadeResult & { id: string }>("new_timeline", { name, rate_num, rate_den });
  }
  link(clips: string[], group: string | null) { return this.call<CascadeResult>("link", { clips, group }); }
  bindClip(clip: string, scene: string) { return this.call<CascadeResult>("bind_clip", { clip, scene }); }
  unbindClip(clip: string, scene: string) { return this.call<CascadeResult>("unbind_clip", { clip, scene }); }
  conformDone(clip: string) { return this.call<CascadeResult>("conform_done", { clip }); }
  script() { return this.call<ScriptView>("script"); }
  impact(revision?: number) { return this.call<ImpactView | null>("impact", revision == null ? {} : { revision }); }
  history(limit = 100) { return this.call<HistoryView>("history", { limit }); }
  pictureAt(timeline: string, time: ExactTime) { return this.call<PictureAt>("picture_at", { timeline, time }); }
  scriptText(document: string) { return this.call<string>("script_text", { document }); }
  scriptWrite(document: string, text: string, allowOrphans = false) {
    return this.call<CascadeResult & { refused: boolean; orphans: { scene: string; heading: string; clips: number; takes: number }[] }>(
      "script_write",
      { document, text, allow_orphans: allowOrphans },
    );
  }
  /** The project's colour config, what is looked through, and each file's colorspace. */
  color() {
    return this.call<ColorView>("color", {});
  }
  /** Look through this display and view everywhere; `null` returns to the config's default. */
  colorView(selection: ViewSelection | null) {
    return this.call<ColorView>("color_view", {
      display: selection?.display ?? null,
      view: selection?.view ?? null,
    });
  }
  /** Grade a clip. The viewer bakes it into the transform it already applies. */
  setGrade(clip: string, cdl: Cdl) {
    return this.call<CascadeResult>("set_grade", {
      clip,
      slope: cdl.slope,
      offset: cdl.offset,
      power: cdl.power,
      saturation: cdl.saturation,
    });
  }
  /** Say what a file really is. Undoable, like any other edit. */
  setInputColorspace(media: string, colorspace: string) {
    return this.call<CascadeResult>("set_input_colorspace", { media, colorspace });
  }
  production(document?: string) {
    return this.call<ProductionView>("production", document ? { document } : {});
  }
  scheduleConstraints(document: string, constraints: ScheduleConstraints) {
    return this.call<CascadeResult>("schedule_constraints", { document, constraints });
  }
  /** Issue revised pages: snapshot the script, so everything after it is marked against it. */
  issueRevision(document: string, date?: string, note?: string) {
    return this.call<CascadeResult & { revision: { label: string; colour: string } }>(
      "issue_revision",
      { document, date, note },
    );
  }
  /**
   * Tag a breakdown element by hand: `category` is `prop`, `cast`, `vehicles`,
   * `animals`, `sfx` or `vfx`. `present` undefined hands the category back to
   * the script.
   */
  tagBreakdown(scene: string, category: string, value: string, present?: boolean) {
    return this.call<CascadeResult>("tag_breakdown", { scene, category, value, present });
  }
  /** Put a scene on a shooting day, or `null` to let the scheduler decide again. */
  moveStrip(document: string, scene: string, day: number | null) {
    return this.call<CascadeResult>("move_strip", { document, scene, day });
  }
  jobs(limit = 100) { return this.call<JobView[]>("jobs", { limit }); }
  submitJob(kind: string, params: Record<string, unknown>) { return this.call<JobView>("job_submit", { kind, ...params }); }
  cancelJob(id: number) { return this.call<JobView>("job_cancel", { id }); }
  retryJob(id: number) { return this.call<JobView>("job_retry", { id }); }
  focus(timeline?: string | null) {
    return this.call<{ timeline: string | null }>("focus", timeline === undefined ? {} : { timeline });
  }
  importFountain(text: string, key?: string) { return this.call<unknown>("import_fountain", { text, key }); }
}
