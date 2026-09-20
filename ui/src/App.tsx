import { useCallback, useEffect, useMemo, useState } from "react";
import { Browser } from "./panels/Browser";
import { Rail } from "./panels/Rail";
import { useSubject } from "./state/subject";
import { Capabilities } from "./panels/Capabilities";
import { History, Impact } from "./panels/Impact";
import { Jobs } from "./panels/Jobs";
import { Captions } from "./panels/Captions";
import { Colour } from "./panels/Colour";
import { Production } from "./panels/Production";
import { Inspector } from "./panels/Inspector";
import { Record } from "./panels/Record";
import { Script } from "./panels/Script";
import { Writer } from "./panels/Writer";
import { Timeline, itemKey } from "./panels/Timeline";
import { Viewer } from "./panels/Viewer";
import type {
  EditMarks,
  EditOp,
  ExactTime,
  MarkMismatch,
  MediaPreference,
  MediaView,
  TimelineItem,
  TimelineTrack,
  TimelineView,
} from "./rpc/client";
import {
  Tool,
  TOOLS,
  exact,
  frameAt,
  frameLabel,
  markSecondsOf,
  sequenceRate,
  seconds,
  spanOf,
} from "./editing/edit";
import { useCaptions } from "./state/captions";
import { useSession } from "./state/session";
import { WORKSPACES, canOpen, workspaceReason, workspaceState } from "./workspaces";

type Selection = { item: TimelineItem; track: TimelineTrack } | null;
type Dialog = null | "sequence" | "media" | "screenplay";

const NO_MARKS = { in: null, out: null };

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
  // The lens: which craft's view is open. It changes freely and is never an
  // input to what the subject is, which is what makes the subject survive it.
  const [workspace, setWorkspace] = useState("editorial");
  const [selectionKey, setSelectionKey] = useState<string | null>(null);
  const [mediaId, setMediaId] = useState<string | null>(null);
  const [picture, setPicture] = useState(0);
  const [marks, setMarks] = useState<Record<string, { in: number | null; out: number | null }>>({});
  // Record marks, per sequence, in ticks at that sequence's rate. They are
  // marks, not project state: nobody else's cut depends on where this editor
  // is about to put something.
  const [recordMarks, setRecordMarks] = useState<Record<string, { in: number | null; out: number | null }>>({});
  const [mismatch, setMismatch] = useState<MarkMismatch>("refuse");
  const [captionLanguage, setCaptionLanguage] = useState<string | null>(null);
  const [playheadTicks, setPlayheadTicks] = useState(0);
  const [tool, setTool] = useState<Tool>("select");
  const [snapping, setSnapping] = useState(true);
  const [linked, setLinked] = useState(true);
  const [monitor, setMonitor] = useState<"source" | "record">("record");
  const [lastApplied, setLastApplied] = useState<string | null>(null);
  const [sceneId, setSceneId] = useState<string | null>(null);
  const [rightTab, setRightTab] = useState<"inspector" | "captions" | "history" | "jobs">("inspector");
  const [dialog, setDialog] = useState<Dialog>(null);
  const [scriptMode, setScriptMode] = useState<"read" | "write">("read");
  // What is being worked on. Application state: where a person is looking is
  // not a fact about the film, so it is not written into the project graph.
  const subject = useSubject(session.client, session.project?.revision ?? null);

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

  /**
   * A three-point edit: send the marks, let the runtime do the arithmetic.
   *
   * The browser decides nothing here. Which point is derived, whether the
   * edit is backtimed, whether four marks can be reconciled and how far the
   * playhead then moves all come back in the plan, because all of them are
   * exact rational time and computing them here would be a second, drifting
   * implementation of the same arithmetic.
   */
  const place = useCallback(
    async (mode: "insert" | "overwrite") => {
      if (!timeline || !media?.video || !playhead) return;
      const m = marks[media.id] ?? { in: null, out: null };
      const mediaRate = { num: media.video.rate_num ?? 0, den: media.video.rate_den ?? 0 };
      if (!mediaRate.num || !mediaRate.den) {
        session.run(mode, () => Promise.reject(new Error("the media declares no frame rate")));
        return;
      }
      const tracks = destinationTracks(timeline, media);
      if (tracks.length === 0) {
        session.run(mode, () => Promise.reject(new Error("no unlocked track to place material on")));
        return;
      }
      // A mark is the picture the editor is looking at; an out point is one
      // past the last picture they want, which is where the two models meet.
      const source: EditMarks = {
        in: m.in == null ? null : exact(m.in, mediaRate),
        out: m.out == null ? null : exact(m.out + 1, mediaRate),
      };
      const rm = recordMarks[timeline.id] ?? { in: null, out: null };
      const record: EditMarks =
        rm.in == null && rm.out == null
          ? { in: playhead, out: null }
          : {
              in: rm.in == null ? null : exact(rm.in, rate),
              out: rm.out == null ? null : exact(rm.out + 1, rate),
            };
      const r = await session.run(mode, () =>
        client.threePoint(tracks, media.id, source, record, mode, mismatch),
      );
      if (!r) return;
      setLastApplied(`${r.plan.description} · ${media.name} on ${tracks.length} track(s)`);
      // As in every editor, the playhead moves to the end of what was placed.
      setPlayheadTicks(frameAt(r.plan.record.end.seconds, rate));
      if (r.clips[0]) setSelectionKey(r.clips[0]);
    },
    [client, marks, media, mismatch, playhead, rate, recordMarks, session, timeline],
  );

  /**
   * A multicam switch: cut at the playhead and play the angle from there.
   *
   * `1`..`9` pick the angle by its place in the clip's own list, which is what
   * the number keys have always meant in a cutting room. The runtime decides
   * whether that angle has the media; if it does not it refuses by name and
   * nothing is cut.
   */
  const switchAngle = useCallback(
    (position: number) => {
      if (!timeline || !playhead) return;
      const under = timeline.tracks
        .filter((t) => t.kind === "video" && !t.locked)
        .flatMap((t) =>
          t.items
            .filter((i) => i.kind === "clip" && i.angles)
            .map((i) => ({ track: t, item: i })),
        )
        .find(
          ({ item }) =>
            item.at.start.ticks <= playheadTicks && playheadTicks < item.at.end.ticks,
        );
      if (!under?.track.id) return;
      const key = under.item.angles?.[position - 1];
      if (!key) return;
      void session.run(`angle ${key}`, () => client.switchAngle(under.track.id!, playhead, key));
    },
    [client, playhead, playheadTicks, session, timeline],
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

  /**
   * I, O and clear apply to whichever monitor is active — the source viewer
   * marks pictures of the media, the record monitor marks the sequence. That
   * is what those keys have meant in every cutting room since tape.
   */
  const mark = useCallback(
    (which: "in" | "out" | "clear") => {
      if (monitor === "record") {
        if (!timeline) return;
        setRecordMarks((all) => {
          const cur = all[timeline.id] ?? { in: null, out: null };
          const next = which === "clear" ? { in: null, out: null } : { ...cur, [which]: playheadTicks };
          return { ...all, [timeline.id]: next };
        });
        return;
      }
      if (!media) return;
      setMarks((all) => {
        const cur = all[media.id] ?? { in: null, out: null };
        const next = which === "clear" ? { in: null, out: null } : { ...cur, [which]: picture };
        return { ...all, [media.id]: next };
      });
    },
    [media, monitor, picture, playheadTicks, timeline],
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
    angle: switchAngle,
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
  const captions = useCaptions(client, timeline?.id ?? null, captionLanguage, revision, setCaptionLanguage);

  // The band the timeline draws. Ticks are the number; seconds are layout.
  const recordMarkSeconds = useMemo(
    () => markSecondsOf(timeline ? (recordMarks[timeline.id] ?? NO_MARKS) : NO_MARKS, rate),
    [rate, recordMarks, timeline],
  );

  // One place decides what selecting a thing means. A subject is the graph's
  // node; the existing per-surface selections follow from it rather than
  // competing with it.
  const chooseSubject = useCallback(
    (entry: { id: string; kind: string; lens: string | null }) => {
      subject.select({ id: entry.id, kind: entry.kind });
      if (entry.kind === "asset.media") {
        setMediaId(entry.id);
        setPicture(0);
        setMonitor("source");
      } else if (entry.kind === "nle.timeline") {
        session.selectTimeline(entry.id);
        setPlayheadTicks(0);
      } else if (entry.kind === "script.scene") {
        setSceneId(entry.id);
      }
    },
    [session, subject],
  );

  const rail = (
    <Rail
      view={subject.view}
      loading={subject.loading}
      refusal={subject.refusal}
      lens={workspace}
      onSubject={chooseSubject}
      onLens={setWorkspace}
    />
  );

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
        subject.select({ id, kind: "nle.timeline" });
      }}
      onMedia={(id) => {
        setMediaId(id);
        setPicture(0);
        setMonitor("source");
        setSelectionKey(null);
        subject.select({ id, kind: "asset.media" });
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
          <div className="left-stack">
            {rail}
            {browser}
          </div>
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
            preference={session.mediaPreference}
            active={monitor === "record"}
            onActivate={() => setMonitor("record")}
            onStep={stepRecord}
            onSwitchAngle={(key) => {
              if (!timeline || !playhead) return;
              const track = timeline.tracks.find(
                (t) =>
                  t.kind === "video" &&
                  t.items.some(
                    (i) => i.angles && i.at.start.ticks <= playheadTicks && playheadTicks < i.at.end.ticks,
                  ),
              );
              if (track?.id) void session.run(`angle ${key}`, () => client.switchAngle(track.id!, playhead, key));
            }}
          />
          <div className="right-stack">
            <div className="panel tabs-panel">
              <div className="panel-head tabs">
                <button aria-selected={rightTab === "inspector"} onClick={() => setRightTab("inspector")}>
                  Inspector
                </button>
                <button aria-selected={rightTab === "captions"} onClick={() => setRightTab("captions")}>
                  Captions
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
                  onRelink={(media, path) =>
                    void session.run("relink", () => client.relink(media, path)).then(() => session.refresh())
                  }
                  onRetime={(clip, played, reversed, keepPitch, ramp) =>
                    void session.run(ramp ? "speed ramp" : "retime", () =>
                      client.retime(clip, played, reversed, keepPitch, ramp ?? null),
                    )
                  }
                  onAngle={(clip, key) =>
                    void session.run(`angle ${key}`, () => client.setAngle(clip, key))
                  }
                />
              ) : rightTab === "captions" ? (
                <Captions
                  client={client}
                  timeline={timeline}
                  view={captions}
                  language={captionLanguage}
                  onLanguage={setCaptionLanguage}
                  playhead={playhead}
                  marks={timeline ? (recordMarks[timeline.id] ?? NO_MARKS) : NO_MARKS}
                  onSeek={(ticks) => {
                    setPlayheadTicks(ticks);
                    setMonitor("record");
                  }}
                  run={session.run}
                />
              ) : rightTab === "jobs" ? (
                <Jobs
                  jobs={session.jobs}
                  onCancel={(id) => void session.run("cancel job", () => client.cancelJob(id))}
                  onRetry={(id) => void session.run("retry job", () => client.retryJob(id))}
                  render={
                    timeline
                      ? {
                          client,
                          revision,
                          timeline,
                          languages: captions?.languages ?? [],
                          marks: recordMarks[timeline.id] ?? NO_MARKS,
                          onRender: (params) =>
                            void session.run("render", () => client.submitJob("render.sequence", params)),
                        }
                      : undefined
                  }
                />
              ) : (
                <History history={session.history} onUndo={() => void session.undo()} onRedo={() => void session.redo()} />
              )}
            </div>
            {impact}
          </div>
          <Timeline
            timeline={timeline}
              client={client}
              revision={revision}
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
            marks={recordMarkSeconds}
            onMark={(which) => {
              setMonitor("record");
              if (which === "clear") {
                if (timeline) setRecordMarks((all) => ({ ...all, [timeline.id]: { in: null, out: null } }));
                return;
              }
              if (!timeline) return;
              setRecordMarks((all) => {
                const cur = all[timeline.id] ?? { in: null, out: null };
                return { ...all, [timeline.id]: { ...cur, [which]: playheadTicks } };
              });
            }}
            mismatch={mismatch}
            onMismatch={setMismatch}
            captions={captions?.track ?? null}
            onCaption={(ticks) => {
              setPlayheadTicks(ticks);
              setMonitor("record");
              setRightTab("captions");
            }}
          />
        </div>
      )}

      {workspace === "script" && scriptMode === "write" && session.script?.documents[0] && (
        <div className="work script-write">
          <div className="mode-bar">
            <button onClick={() => setScriptMode("read")}>Read &amp; bind</button>
            <button aria-pressed>Write</button>
          </div>
          <Writer
            client={client}
            document={session.script.documents[0].id}
            revision={revision}
            run={session.run}
          />
          {impact}
        </div>
      )}

      {workspace === "script" && (scriptMode === "read" || !session.script?.documents[0]) && (
        <div className="work script">
          <div className="mode-bar">
            <button aria-pressed>Read &amp; bind</button>
            <button disabled={!session.script?.documents[0]} onClick={() => setScriptMode("write")}>
              Write
            </button>
          </div>
          <Script
            script={session.script}
            sceneId={sceneId}
            onScene={setSceneId}
            onIssueRevision={(doc) =>
              void session.run("issue revision", () => client.issueRevision(doc))
            }
            onEditLine={(line, text) => void session.run("edit line", () => client.editDialogue(line, text))}
            selectedClip={selection?.item ?? null}
            onBind={(clip, scene) => void session.run("bind", () => client.bindClip(clip, scene))}
            onUnbind={(clip, scene) => void session.run("unbind", () => client.unbindClip(clip, scene))}
            onRevealClip={revealClip}
          />
          {impact}
        </div>
      )}

      {workspace === "production" && (
        <div className="work production">
          <Production
            production={session.production}
            onConstraints={(doc, c) => void session.run("reschedule", () => client.scheduleConstraints(doc, c))}
            onMoveStrip={(doc, scene, day) =>
              void session.run("move strip", () => client.moveStrip(doc, scene, day))
            }
            onTag={(scene, category, value, present) =>
              void session.run("tag breakdown", () => client.tagBreakdown(scene, category, value, present))
            }
          />
          {impact}
        </div>
      )}

      {workspace === "colour" && (
        <div className="work colour-work">
          <Colour
            colour={session.colour}
            onView={(selection) => void session.run("view", () => client.colorView(selection))}
            onInputColorspace={(media, colorspace) =>
              void session.run("input colorspace", () => client.setInputColorspace(media, colorspace))
            }
            onGrade={(clip, cdl) => void session.run("grade", () => client.setGrade(clip, cdl))}
          />
          <Viewer media={media} capabilities={session.capabilities} index={picture} onIndex={setPicture} />
          {impact}
        </div>
      )}

      {workspace === "media" && (
        <div className="work media">
          <div className="left-stack">
            {rail}
            {browser}
          </div>
          <Viewer media={media} capabilities={session.capabilities} index={picture} onIndex={setPicture} />
          <div className="right-stack">
            <div className="panel tabs-panel">
              <div className="panel-head">Media</div>
              <Inspector
                timeline={null}
                selection={null}
                media={media}
                onJob={(kind, params) => void session.run(kind, () => client.submitJob(kind, params))}
                onRelink={(media, path) =>
                  void session.run("relink", () => client.relink(media, path)).then(() => session.refresh())
                }
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

      <MediaPreferenceSwitch session={session} />
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
  const [asSequence, setAsSequence] = useState(false);
  // An hour is what a reel starts at nearly everywhere; it is offered rather
  // than assumed, so nothing is decided on a person's behalf out of sight.
  const [startTimecode, setStartTimecode] = useState("01:00:00:00");
  const [withThumbnail, setWithThumbnail] = useState(true);
  const [withWaveform, setWithWaveform] = useState(true);
  const [withProxy, setWithProxy] = useState(false);
  const [proxyHeight, setProxyHeight] = useState(540);
  const [imported, setImported] = useState<string | null>(null);
  const { client } = session;

  const submit = async () => {
    setBusy(true);
    let ok: unknown = null;
    if (kind === "sequence") {
      const [n, d] = fps.split("/").map(Number);
      ok = await session.run("new sequence", () =>
        client.newTimeline(name, n, d || 1, startTimecode.trim() || undefined),
      );
      if (ok && typeof ok === "object" && "id" in ok) session.selectTimeline((ok as { id: string }).id);
    } else if (kind === "media") {
      const [n, d] = fps.split("/").map(Number);
      // An import is a job: it reports its own progress in Jobs, and the
      // derived work it starts reports its own.
      ok = await session.run("import media", () =>
        client.importMedia(text.trim(), {
          sequence: asSequence ? true : undefined,
          rate: asSequence ? { num: n, den: d || 1 } : undefined,
          thumbnail: withThumbnail,
          waveform: withWaveform,
          proxyHeight: withProxy ? proxyHeight : null,
        }),
      );
      if (ok && typeof ok === "object" && "label" in ok) {
        setImported(`${(ok as { label: string }).label} — watch it in Jobs`);
        // An import is a job, and cards arrive one after another: the dialog
        // stays, says what was submitted, and offers the next path.
        setText("");
        setBusy(false);
        return;
      }
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
              <label>
                Start timecode
                <input
                  aria-label="Sequence start timecode"
                  value={startTimecode}
                  onChange={(e) => setStartTimecode(e.target.value)}
                  placeholder="01:00:00:00"
                />
              </label>
              <p className="hint">
                Rates are exact ratios; 23.976 is stored as 24000/1001, never as a decimal. The start
                timecode is what the cut&rsquo;s first frame is called — an EDL&rsquo;s record columns and a
                delivered movie count from it. It is read at the sequence&rsquo;s own rate, so at 29.97
                <span className="mono"> 01:00:00;00</span> and <span className="mono">01:00:00:00</span> are
                two different frames; write the semicolon where you mean drop frame. Leave it empty to
                start at zero.
              </p>
            </>
          )}
          {kind === "media" && (
            <>
              <label>
                File path on the machine running the project
                <input value={text} onChange={(e) => setText(e.target.value)} placeholder="/footage/A001_C003.mp4" autoFocus />
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  aria-label="Import as sequence"
                  checked={asSequence}
                  onChange={(e) => setAsSequence(e.target.checked)}
                />
                Read as an image sequence (a numbered run of EXR, PNG or TIFF files)
              </label>
              {asSequence && (
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
                    <option value="60/1">60</option>
                  </select>
                </label>
              )}
              <div className="section-title">After importing</div>
              <label className="check">
                <input
                  type="checkbox"
                  aria-label="Make thumbnail"
                  checked={withThumbnail}
                  onChange={(e) => setWithThumbnail(e.target.checked)}
                />
                Thumbnail
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  aria-label="Make waveform"
                  checked={withWaveform}
                  onChange={(e) => setWithWaveform(e.target.checked)}
                />
                Waveform
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  aria-label="Make proxy"
                  checked={withProxy}
                  onChange={(e) => setWithProxy(e.target.checked)}
                />
                Proxy
                {withProxy && (
                  <select
                    aria-label="Import proxy height"
                    value={proxyHeight}
                    onChange={(e) => setProxyHeight(Number(e.target.value))}
                  >
                    {[1080, 720, 540, 360, 180].map((hgt) => (
                      <option key={hgt} value={hgt}>
                        {hgt}p
                      </option>
                    ))}
                  </select>
                )}
              </label>
              <p className="hint">
                The file is probed and identified by its content. Codecs this build cannot decode are recorded
                as such, not hidden. A run of files has no rate of its own, so a sequence takes the one chosen
                here; a number inside the run with no file is a hole, and is reported rather than shown black.
                Thumbnails and waveforms are quick; a proxy is minutes of work and gigabytes of disk, so it
                happens only when asked for.
              </p>
              {imported && <div className="notice">Imported {imported}</div>}
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
          <button onClick={onClose}>{imported ? "Close" : "Cancel"}</button>
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
  angle(position: number): void;
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
      } else if (key >= "1" && key <= "9" && !e.shiftKey && !e.altKey) {
        on.angle(Number(key));
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

/**
 * Which file every surface reads: the camera originals, proxies where there
 * are current ones, or a decision per clip. Shared by the whole application,
 * like the open sequence — choosing here changes the native window too. It is
 * a viewing choice: renders always read the originals.
 */
function MediaPreferenceSwitch({ session }: { session: ReturnType<typeof useSession> }) {
  const preference = session.mediaPreference;
  return (
    <div className="media-preference">
      <label>
        Play{" "}
        <select
          aria-label="Media"
          value={preference}
          onChange={(e) => void session.setMediaPreference(e.target.value as MediaPreference)}
        >
          <option value="original">Originals</option>
          <option value="proxy">Proxies</option>
          <option value="auto">Auto</option>
        </select>
      </label>
      <button title="Look for every original on disk now" onClick={() => void session.checkMedia()}>
        Check media
      </button>
    </div>
  );
}
