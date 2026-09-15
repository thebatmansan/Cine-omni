import { useCallback, useEffect, useMemo, useState } from "react";
import { Browser } from "./panels/Browser";
import { Capabilities } from "./panels/Capabilities";
import { Inspector } from "./panels/Inspector";
import { Timeline, itemKey } from "./panels/Timeline";
import { Viewer } from "./panels/Viewer";
import { TimelineItem, TimelineTrack } from "./rpc/client";
import { useSession } from "./state/session";
import { WORKSPACES, canOpen, workspaceReason, workspaceState } from "./workspaces";

/**
 * The application shell.
 *
 * A menu bar that names the project, workspace navigation across the whole
 * intended application, the working area, and a status bar carrying the one
 * number that matters for coherence: the revision every client shares.
 */
export function App() {
  const session = useSession();
  const [workspace, setWorkspace] = useState("editorial");
  const [selection, setSelection] = useState<{ item: TimelineItem; track: TimelineTrack } | null>(
    null,
  );
  const [mediaId, setMediaId] = useState<string | null>(null);
  const [picture, setPicture] = useState(0);
  const [playhead, setPlayhead] = useState(0);

  const media = useMemo(
    () => session.media.find((m) => m.id === mediaId) ?? session.media[0] ?? null,
    [session.media, mediaId],
  );

  // Selecting a clip moves the playhead to its start, which is what clicking a
  // clip means in every editor.
  const onSelect = useCallback((item: TimelineItem, track: TimelineTrack) => {
    setSelection({ item, track });
    setPlayhead(item.at.start.seconds);
  }, []);

  useKeyboard({
    onStep: (by) => setPicture((p) => Math.max(0, p + by)),
    onHome: () => {
      setPicture(0);
      setPlayhead(0);
    },
    undo: session.undo,
    redo: session.redo,
    save: session.save,
  });

  const selectedKey = selection ? itemKey(selection.item, selection.track) : null;

  return (
    <div className="shell">
      <MenuBar session={session} />
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
          <Browser
            timelines={session.timelines}
            structure={session.structure}
            media={session.media}
            selectedTimeline={session.timeline?.id ?? null}
            selectedMedia={media?.id ?? null}
            onTimeline={session.selectTimeline}
            onMedia={setMediaId}
          />
          <Viewer
            media={media}
            capabilities={session.capabilities}
            index={picture}
            onIndex={setPicture}
          />
          <Inspector timeline={session.timeline} selection={selection} media={media} />
          <Timeline
            timeline={session.timeline}
            selected={selectedKey}
            onSelect={onSelect}
            playhead={playhead}
            onScrub={setPlayhead}
            onTrackState={(track, key, value) => void session.setTrackState(track, key, value)}
          />
        </div>
      )}

      {workspace === "media" && (
        <div className="work media">
          <Browser
            timelines={session.timelines}
            structure={session.structure}
            media={session.media}
            selectedTimeline={session.timeline?.id ?? null}
            selectedMedia={media?.id ?? null}
            onTimeline={session.selectTimeline}
            onMedia={setMediaId}
          />
          <Viewer
            media={media}
            capabilities={session.capabilities}
            index={picture}
            onIndex={setPicture}
          />
          <Capabilities capabilities={session.capabilities} />
        </div>
      )}

      <StatusBar session={session} />
    </div>
  );
}

function MenuBar({ session }: { session: ReturnType<typeof useSession> }) {
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

  const entry = (label: string, shortcut: string, run: () => void, why?: string) => (
    <button
      onClick={() => {
        run();
        setOpen(null);
      }}
      disabled={!!why}
      title={why ?? ""}
    >
      <span>{label}</span>
      <span className="shortcut">{shortcut}</span>
    </button>
  );

  const notWired = "Implemented in the engine; no protocol reaches it from here yet.";

  return (
    <div className="menubar">
      <span className="brand">CINE-OMNI</span>
      {menu(
        "Project",
        <>
          {entry("Save", "⌘S", () => void session.save())}
          <div className="menu-sep" />
          {entry("Export screenplay…", "", () => {}, notWired)}
        </>,
      )}
      {menu(
        "Edit",
        <>
          {entry("Undo", "⌘Z", () => void session.undo())}
          {entry("Redo", "⇧⌘Z", () => void session.redo())}
          <div className="menu-sep" />
          {entry("Ripple trim", "", () => {}, notWired)}
          {entry("Roll", "", () => {}, notWired)}
          {entry("Slip", "", () => {}, notWired)}
          {entry("Slide", "", () => {}, notWired)}
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
      {session.error && <span style={{ color: "var(--bad)" }}>{session.error}</span>}
    </div>
  );
}

/** Keyboard model: the few things that are genuinely wired. */
function useKeyboard(on: {
  onStep(by: number): void;
  onHome(): void;
  undo(): Promise<void>;
  redo(): Promise<void>;
  save(): Promise<void>;
}) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)) return;
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.key.toLowerCase() === "z") {
        e.preventDefault();
        void (e.shiftKey ? on.redo() : on.undo());
      } else if (meta && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void on.save();
      } else if (e.key === "ArrowRight") {
        on.onStep(1);
      } else if (e.key === "ArrowLeft") {
        on.onStep(-1);
      } else if (e.key === "Home") {
        on.onHome();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [on]);
}
