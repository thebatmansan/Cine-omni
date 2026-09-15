import { useCallback, useEffect, useMemo, useState } from "react";
import { Browser } from "./panels/Browser";
import { Capabilities } from "./panels/Capabilities";
import { History, Impact } from "./panels/Impact";
import { Jobs } from "./panels/Jobs";
import { Inspector } from "./panels/Inspector";
import { Record } from "./panels/Record";
import { Script } from "./panels/Script";
import { Timeline, itemKey } from "./panels/Timeline";
import { Viewer } from "./panels/Viewer";
import type { EditOp, ExactTime, MediaView, TimelineItem, TimelineTrack, TimelineView } from "./rpc/client";
import { Tool, TOOLS, exact, frameAt, frameLabel, sequenceRate, seconds, spanOf } from "./editing/edit";
import { useSession } from "./state/session";
import { WORKSPACES, canOpen, workspaceReason, workspaceState } from "./workspaces";

type Selection = { item: TimelineItem; track: TimelineTrack } | null;
type Dialog = null | "sequence" | "media" | "screenplay";

/**
 * The application shell.
 *
 * A menu bar that names the project, workspace navigation across the whole
 * intended application, the working area, and a status bar carrying the one
 * number that matters for coherence: the revision every client shares.
 *
 * Everything held here is *editor* state — which tool, which monitor is active,
 * where the playhead is, what is marked on the source. None of it is project
 * state, and none of it is sent anywhere until it becomes an edit.
 */
export function App() {
  const session = useSession();
  const { client, timeline } = session;
  const [workspace, setWorkspace] = useState("editorial");
  const [selectionKey, setSelectionKey] = useState<string | null>(null);
  const [mediaId, setMediaId] = useState<string | null>(null);
  const [picture, setPicture] = useState(0);
  const [marks, setMarks] = useState<Record<string, { in: number | null; out: number | null }>>({});
  const [playheadTicks, setPlayheadTicks] = useState(0);
  const [tool, setTool] = useState<Tool>("select");
  const [snapping, setSnapping] = useState(true);
  const [linked, setLinked] = useState(true);
  const [monitor, setMonitor] = useState<"source" | "record">("record");
  const [lastApplied, setLastApplied] = useState<string | null>(null);
  const [sceneId, setSceneId] = useState<string | null>(null);
  const [rightTab, setRightTab] = useState<"inspector" | "history" | "jobs">("inspector");
  const [dialog, setDialog] = useState<Dialog>(null);

  const media = useMemo(
    () => session.media.find((m) => m.id === mediaId) ?? session.media.find((m) => m.video?.decodable) ?? null,
    [session.media, mediaId],
  );
  const rate = timeline ? sequenceRate(timeline) : { num: 24, den: 1 };
  const playhead: ExactTime | null = timeline ? exact(playheadTicks, rate) : null;
  const playheadSeconds = playhead ? seconds(playhead) : 0;

  // The selection is a key; the item is looked up in the current projection so
  // an edit by any client is reflected in what the inspector shows.
  const selection: Selection = useMemo(() => findItem(timeline, selectionKey), [timeline, selectionKey]);

  const onSelect = useCallback((item: TimelineItem, track: TimelineTrack) => {
    setSelectionKey(itemKey(item, track));
    setMonitor("record");
  }, []);

  const edit = useCallback(
    async (track: TimelineTrack, op: EditOp) => {
      if (!track.id) return;
      const r = await session.run(op.op.replace("_", " "), () => client.edit(track.id!, op, linked));
      if (!r) return;
      const o = r.outcome;
      const applied = o.applied ? frameLabel(o.applied.ticks) : null;
      const asked = o.requested ? frameLabel(o.requested.ticks) : null;
      setLastApplied(
        [
          o.label,
          applied && (o.clamped ? `${applied} of ${asked} (media limit)` : applied),
          o.linked_clips ? `+${o.linked_clips} linked` : null,
        ]
          .filter(Boolean)
          .join(" · "),
      );
      if (o.item && op.op !== "split_at") setSelectionKey(o.item);
    },
    [client, linked, session],
  );

  const place = useCallback(
    async (mode: "insert" | "overwrite") => {
      if (!timeline || !media?.video || !playhead) return;
      const m = marks[media.id] ?? { in: null, out: null };
      const mediaRate = { num: media.video.rate_num ?? 0, den: media.video.rate_den ?? 0 };
      if (!mediaRate.num || !mediaRate.den) {
        session.run(mode, () => Promise.reject(new Error("the media declares no frame rate")));
        return;
      }
      const first = m.in ?? 0;
      const last = m.out ?? (media.video.frames ?? 1) - 1;
      if (last < first) {
        session.run(mode, () => Promise.reject(new Error("the out mark is before the in mark")));
        return;
      }
      const tracks = destinationTracks(timeline, media);
      if (tracks.length === 0) {
        session.run(mode, () => Promise.reject(new Error("no unlocked track to place material on")));
        return;
      }
      const clip = {
        media: media.id,
        source_range: { start_time: exact(first, mediaRate), duration: exact(last - first + 1, mediaRate) },
      };
      const r = await session.run(mode, () => client.place(tracks, playhead, clip, mode));
      if (!r) return;
      setLastApplied(`${mode} · ${last - first + 1} pictures of ${media.name} on ${tracks.length} track(s)`);
      // As in every editor, the playhead moves to the end of what was placed.
      const durationSeconds = ((last - first + 1) * mediaRate.den) / mediaRate.num;
      setPlayheadTicks((t) => t + Math.round((durationSeconds * rate.num) / rate.den));
      if (r.clips[0]) setSelectionKey(r.clips[0]);
    },
    [client, marks, media, playhead, rate.den, rate.num, session, timeline],
  );

  const removeSelected = useCallback(
    (how: "lift" | "extract") => {
      if (!selection || selection.item.kind === "transition") return;
      void edit(selection.track, { op: how, range: spanOf(selection.item) });
    },
    [edit, selection],
  );

  const splitAtPlayhead = useCallback(() => {
    if (!timeline || !playhead) return;
    const track =
      selection?.track ?? timeline.tracks.find((t) => t.kind === "video" && t.targeted) ?? timeline.tracks[0];
    if (track) void edit(track, { op: "split_at", time: playhead });
  }, [edit, playhead, selection, timeline]);

  const mark = useCallback(
    (which: "in" | "out" | "clear") => {
      if (!media) return;
      setMarks((all) => {
        const cur = all[media.id] ?? { in: null, out: null };
        const next = which === "clear" ? { in: null, out: null } : { ...cur, [which]: picture };
        return { ...all, [media.id]: next };
      });
    },
    [media, picture],
  );

  const stepRecord = useCallback(
    (by: number) => setPlayheadTicks((t) => Math.max(0, t + by)),
    [],
  );

  useKeyboard({
    step: (by) => (monitor === "source" ? setPicture((p) => Math.max(0, p + by)) : stepRecord(by)),
    home: () => (monitor === "source" ? setPicture(0) : setPlayheadTicks(0)),
    tool: setTool,
    snap: () => setSnapping((s) => !s),
    linked: () => setLinked((l) => !l),
    mark,
    place: (mode) => void place(mode),
    remove: removeSelected,
    split: splitAtPlayhead,
    undo: session.undo,
    redo: session.redo,
    save: session.save,
  });

  const revealClip = useCallback(
    (clip: string) => {
      const found = findItem(timeline, clip);
      if (!found) return;
      setSelectionKey(clip);
      setPlayheadTicks(frameAt(found.item.at.start.seconds, rate));
      setWorkspace("editorial");
    },
    [rate, timeline],
  );

  const revision = session.project?.revision ?? 0;

  const browser = (
    <Browser
      timelines={session.timelines}
      structure={session.structure}
      media={session.media}
      selectedTimeline={timeline?.id ?? null}
      selectedMedia={media?.id ?? null}
      onTimeline={(id) => {
        session.selectTimeline(id);
        setPlayheadTicks(0);
      }}
      onMedia={(id) => {
        setMediaId(id);
        setPicture(0);
        setMonitor("source");
        setSelectionKey(null);
      }}
    />
  );

  const impact = <Impact impact={session.impact} onReveal={revealClip} />;

  return (
    <div className="shell">
      <MenuBar
        session={session}
        onDialog={setDialog}
        onTool={setTool}
        tool={tool}
        canRemove={!!selection && selection.item.kind !== "transition"}
        onRemove={removeSelected}
        onSplit={splitAtPlayhead}
        canPlace={!!timeline && !!media?.video?.decodable}
        onPlace={(m) => void place(m)}
      />
      <nav className="workspaces" role="tablist">
        {WORKSPACES.map((w) => {
          const state = workspaceState(w, session.capabilities);
          const open = canOpen(w, session.capabilities);
          return (
            <button
              key={w.id}
              role="tab"
              aria-selected={workspace === w.id}
              disabled={!open}
              title={open ? w.name : workspaceReason(w, session.capabilities)}
              onClick={() => open && setWorkspace(w.id)}
            >
              <span className={`dot ${state}`} />
              {w.name}
            </button>
          );
        })}
      </nav>

      {workspace === "editorial" && (
        <div className="work editorial">
          {browser}
          <Viewer
            title="Source"
            media={media}
            capabilities={session.capabilities}
            index={picture}
            onIndex={setPicture}
            marks={media ? (marks[media.id] ?? { in: null, out: null }) : undefined}
            onMark={mark}
            active={monitor === "source"}
            onActivate={() => setMonitor("source")}
          />
          <Record
            client={client}
            timeline={timeline}
            playhead={playhead}
            revision={revision}
            active={monitor === "record"}
            onActivate={() => setMonitor("record")}
            onStep={stepRecord}
          />
          <div className="right-stack">
            <div className="panel tabs-panel">
              <div className="panel-head tabs">
                <button aria-selected={rightTab === "inspector"} onClick={() => setRightTab("inspector")}>
                  Inspector
                </button>
                <button aria-selected={rightTab === "history"} onClick={() => setRightTab("history")}>
                  History
                </button>
                <button aria-selected={rightTab === "jobs"} onClick={() => setRightTab("jobs")}>
                  Jobs{session.jobs.some((j) => j.state === "running" || j.state === "queued") ? " ●" : ""}
                </button>
              </div>
              {rightTab === "inspector" ? (
                <Inspector
                  timeline={timeline}
                  selection={selection}
                  media={selection ? null : media}
                  script={session.script}
                  onConformDone={(clip) => void session.run("mark conformed", () => client.conformDone(clip))}
                  onBind={(clip, scene) => void session.run("bind", () => client.bindClip(clip, scene))}
                  onUnbind={(clip, scene) => void session.run("unbind", () => client.unbindClip(clip, scene))}
                  onJob={(kind, params) => void session.run(kind, () => client.submitJob(kind, params))}
                />
              ) : rightTab === "jobs" ? (
                <Jobs
                  jobs={session.jobs}
                  onCancel={(id) => void session.run("cancel job", () => client.cancelJob(id))}
                  onRetry={(id) => void session.run("retry job", () => client.retryJob(id))}
                />
              ) : (
                <History history={session.history} onUndo={() => void session.undo()} onRedo={() => void session.redo()} />
              )}
            </div>
            {impact}
          </div>
          <Timeline
            timeline={timeline}
            selected={selectionKey}
            onSelect={onSelect}
            playheadSeconds={playheadSeconds}
            onScrub={(s) => {
              setPlayheadTicks(frameAt(s, rate));
              setMonitor("record");
            }}
            onTrackState={(track, key, value) => void session.setTrackState(track, key, value)}
            onEdit={(track, op) => void edit(track, op)}
            tool={tool}
            onTool={setTool}
            snapping={snapping}
            onSnapping={setSnapping}
            linked={linked}
            onLinked={setLinked}
            lastApplied={lastApplied}
          />
        </div>
      )}

      {workspace === "script" && (
        <div className="work script">
          <Script
            script={session.script}
            sceneId={sceneId}
            onScene={setSceneId}
            onEditLine={(line, text) => void session.run("edit line", () => client.editDialogue(line, text))}
            selectedClip={selection?.item ?? null}
            onBind={(clip, scene) => void session.run("bind", () => client.bindClip(clip, scene))}
            onUnbind={(clip, scene) => void session.run("unbind", () => client.unbindClip(clip, scene))}
            onRevealClip={revealClip}
          />
          {impact}
        </div>
      )}

      {workspace === "media" && (
        <div className="work media">
          {browser}
          <Viewer media={media} capabilities={session.capabilities} index={picture} onIndex={setPicture} />
          <div className="right-stack">
            <div className="panel tabs-panel">
              <div className="panel-head">Media</div>
              <Inspector
                timeline={null}
                selection={null}
                media={media}
                onJob={(kind, params) => void session.run(kind, () => client.submitJob(kind, params))}
              />
            </div>
            <Jobs
              jobs={session.jobs}
              onCancel={(id) => void session.run("cancel job", () => client.cancelJob(id))}
              onRetry={(id) => void session.run("retry job", () => client.retryJob(id))}
            />
          </div>
          <Capabilities capabilities={session.capabilities} />
        </div>
      )}

      <StatusBar session={session} />
      {dialog && <ProjectDialog kind={dialog} session={session} onClose={() => setDialog(null)} />}
    </div>
  );
}

function findItem(timeline: TimelineView | null, key: string | null): Selection {
  if (!timeline || !key) return null;
  for (const track of timeline.tracks) {
    for (const item of track.items) {
      if (itemKey(item, track) === key) return { item, track };
    }
  }
  return null;
}

/**
 * Where new material goes: the targeted picture track (else the first unlocked
 * one) and, when the media has sound, the targeted sound track (else the first
 * unlocked one). Placed together, they are linked by the runtime.
 */
export function destinationTracks(timeline: TimelineView, media: MediaView): string[] {
  const pick = (kind: "video" | "audio") => {
    const open = timeline.tracks.filter((t) => t.kind === kind && !t.locked && t.id);
    return (open.find((t) => t.targeted) ?? open[0])?.id ?? null;
  };
  const out: string[] = [];
  const v = media.video ? pick("video") : null;
  if (v) out.push(v);
  const hasSound = media.streams.some((s) => s.name.startsWith("audio"));
  const a = hasSound ? pick("audio") : null;
  if (a) out.push(a);
  return out;
}

function MenuBar({
  session,
  onDialog,
  onTool,
  tool,
  canRemove,
  onRemove,
  onSplit,
  canPlace,
  onPlace,
}: {
  session: ReturnType<typeof useSession>;
  onDialog(d: Dialog): void;
  onTool(t: Tool): void;
  tool: Tool;
  canRemove: boolean;
  onRemove(how: "lift" | "extract"): void;
  onSplit(): void;
  canPlace: boolean;
  onPlace(mode: "insert" | "overwrite"): void;
}) {
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    const close = () => setOpen(null);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, []);

  const menu = (name: string, items: React.ReactNode) => (
    <div className={`menu${open === name ? " open" : ""}`}>
      <button
        onMouseDown={(e) => {
          e.stopPropagation();
          setOpen(open === name ? null : name);
        }}
      >
        {name}
      </button>
      {open === name && (
        <div className="menu-items" onMouseDown={(e) => e.stopPropagation()}>
          {items}
        </div>
      )}
    </div>
  );

  const entry = (label: string, shortcut: string, run: () => void, why?: string | false) => (
    <button
      key={label}
      onClick={() => {
        run();
        setOpen(null);
      }}
      disabled={!!why}
      title={why || ""}
    >
      <span>{label}</span>
      <span className="shortcut">{shortcut}</span>
    </button>
  );

  return (
    <div className="menubar">
      <span className="brand">CINE-OMNI</span>
      {menu(
        "Project",
        <>
          {entry("New sequence…", "", () => onDialog("sequence"))}
          {entry("Import media…", "", () => onDialog("media"))}
          {entry("Import screenplay…", "", () => onDialog("screenplay"))}
          <div className="menu-sep" />
          {entry("Save", "⌘S", () => void session.save())}
        </>,
      )}
      {menu(
        "Edit",
        <>
          {entry("Undo", "⌘Z", () => void session.undo(), !session.history?.undo_depth && "Nothing to undo")}
          {entry("Redo", "⇧⌘Z", () => void session.redo(), !session.history?.redo_depth && "Nothing to redo")}
          <div className="menu-sep" />
          {entry("Insert source at playhead", ",", () => onPlace("insert"), !canPlace && "Needs a sequence and decodable source media")}
          {entry("Overwrite source at playhead", ".", () => onPlace("overwrite"), !canPlace && "Needs a sequence and decodable source media")}
          {entry("Split at playhead", "⌘K", onSplit)}
          {entry("Lift selection", "⌫", () => onRemove("lift"), !canRemove && "Select a clip or gap")}
          {entry("Extract selection", "⇧⌫", () => onRemove("extract"), !canRemove && "Select a clip or gap")}
          <div className="menu-sep" />
          {TOOLS.map((t) => entry(`${tool === t.id ? "● " : ""}${t.label} tool`, t.key.toUpperCase(), () => onTool(t.id)))}
        </>,
      )}
      {menu("View", <>{entry("Refresh", "", () => void session.refresh())}</>)}

      <div className="project-id">
        <strong>{session.project?.name ?? "—"}</strong>
        <span className="path">{session.project?.path ?? ""}</span>
      </div>
    </div>
  );
}

function ProjectDialog({
  kind,
  session,
  onClose,
}: {
  kind: Exclude<Dialog, null>;
  session: ReturnType<typeof useSession>;
  onClose(): void;
}) {
  const [name, setName] = useState("Sequence");
  const [fps, setFps] = useState("24/1");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const { client } = session;

  const submit = async () => {
    setBusy(true);
    let ok: unknown = null;
    if (kind === "sequence") {
      const [n, d] = fps.split("/").map(Number);
      ok = await session.run("new sequence", () => client.newTimeline(name, n, d || 1));
      if (ok && typeof ok === "object" && "id" in ok) session.selectTimeline((ok as { id: string }).id);
    } else if (kind === "media") {
      ok = await session.run("import media", () => client.ingest(text.trim()));
    } else {
      ok = await session.run("import screenplay", () => client.importFountain(text, name || "screenplay.fountain"));
    }
    setBusy(false);
    if (ok) onClose();
  };

  return (
    <div className="dialog-backdrop" onMouseDown={onClose}>
      <div className="dialog" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label={kind}>
        <div className="panel-head">
          {kind === "sequence" ? "New sequence" : kind === "media" ? "Import media" : "Import screenplay"}
        </div>
        <div className="dialog-body">
          {kind === "sequence" && (
            <>
              <label>
                Name
                <input value={name} onChange={(e) => setName(e.target.value)} autoFocus />
              </label>
              <label>
                Frame rate
                <select value={fps} onChange={(e) => setFps(e.target.value)}>
                  <option value="24000/1001">23.976 (24000/1001)</option>
                  <option value="24/1">24</option>
                  <option value="25/1">25</option>
                  <option value="30000/1001">29.97 (30000/1001)</option>
                  <option value="30/1">30</option>
                  <option value="48/1">48</option>
                  <option value="50/1">50</option>
                  <option value="60000/1001">59.94 (60000/1001)</option>
                  <option value="60/1">60</option>
                </select>
              </label>
              <p className="hint">Rates are exact ratios; 23.976 is stored as 24000/1001, never as a decimal.</p>
            </>
          )}
          {kind === "media" && (
            <>
              <label>
                File path on the machine running the project
                <input value={text} onChange={(e) => setText(e.target.value)} placeholder="/footage/A001_C003.mp4" autoFocus />
              </label>
              <p className="hint">
                The file is probed and identified by its content. Codecs this build cannot decode are recorded
                as such, not hidden.
              </p>
            </>
          )}
          {kind === "screenplay" && (
            <>
              <label>
                Source name
                <input value={name} onChange={(e) => setName(e.target.value)} />
              </label>
              <label>
                Fountain text
                <textarea value={text} onChange={(e) => setText(e.target.value)} rows={12} autoFocus />
              </label>
            </>
          )}
          {session.refusal && <div className="notice bad">{session.refusal}</div>}
        </div>
        <div className="dialog-actions">
          <button onClick={onClose}>Cancel</button>
          <button className="primary" disabled={busy} onClick={() => void submit()}>
            {kind === "sequence" ? "Create" : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}

function StatusBar({ session }: { session: ReturnType<typeof useSession> }) {
  const p = session.project;
  const last = session.changes[0];
  return (
    <div className="statusbar">
      <span className={`conn${session.connected ? " on" : ""}`}>
        <span className="dot" />
        {session.connected ? "runtime attached" : "no runtime"}
      </span>
      {p && (
        <>
          <span className="mono">rev {p.revision}</span>
          <span className="mono">
            {p.nodes} nodes · {p.edges} edges
          </span>
          <span className="mono">
            {p.clients} client{p.clients === 1 ? "" : "s"}
          </span>
          <span className="mono">undo {p.undo_depth}</span>
        </>
      )}
      {last && (
        <span title={`touched ${last.touched.length} entities`}>
          last: {last.label} → rev {last.revision}
        </span>
      )}
      <span className="spacer" />
      {session.refusal && <span className="refusal" title={session.refusal}>refused — {session.refusal}</span>}
      {session.error && <span style={{ color: "var(--bad)" }}>{session.error}</span>}
    </div>
  );
}

/** Keyboard model. Every key here does something real. */
function useKeyboard(on: {
  step(by: number): void;
  home(): void;
  tool(t: Tool): void;
  snap(): void;
  linked(): void;
  mark(which: "in" | "out" | "clear"): void;
  place(mode: "insert" | "overwrite"): void;
  remove(how: "lift" | "extract"): void;
  split(): void;
  undo(): Promise<void>;
  redo(): Promise<void>;
  save(): Promise<void>;
}) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)) return;
      const meta = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      if (meta && key === "z") {
        e.preventDefault();
        void (e.shiftKey ? on.redo() : on.undo());
      } else if (meta && key === "s") {
        e.preventDefault();
        void on.save();
      } else if (meta && key === "k") {
        e.preventDefault();
        on.split();
      } else if (meta) {
        return;
      } else if (e.key === "ArrowRight") {
        on.step(e.shiftKey ? 10 : 1);
      } else if (e.key === "ArrowLeft") {
        on.step(e.shiftKey ? -10 : -1);
      } else if (e.key === "Home") {
        on.home();
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        on.remove(e.shiftKey ? "extract" : "lift");
      } else if (key === "i") {
        on.mark("in");
      } else if (key === "o") {
        on.mark("out");
      } else if (key === "x" && e.altKey) {
        on.mark("clear");
      } else if (e.key === ",") {
        on.place("insert");
      } else if (e.key === ".") {
        on.place("overwrite");
      } else if (key === "s") {
        on.snap();
      } else if (key === "l" && e.shiftKey) {
        on.linked();
      } else {
        const t = TOOLS.find((x) => x.key === key);
        if (t) on.tool(t.id);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [on]);
}
