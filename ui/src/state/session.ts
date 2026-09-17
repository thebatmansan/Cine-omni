import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Capability,
  ChangeNotification,
  ColorView,
  HistoryView,
  ImpactView,
  JobView,
  MediaPreference,
  ProductionView,
  MediaView,
  ProjectView,
  RpcClient,
  ScriptView,
  StructureView,
  TimelineSummary,
  TimelineView,
  TrackStateKey,
} from "../rpc/client";

/**
 * The application's view of the project.
 *
 * Deliberately a *projection*, not a model. Nothing here is edited locally and
 * then pushed: every mutation is an RPC call, and what comes back is whatever
 * the engine now says. The runtime broadcasts `changed` after each commit —
 * including commits made by other clients — and that is the only thing that
 * causes a refetch.
 *
 * The consequence is the one the architecture is for: two windows cannot
 * disagree, because neither of them is keeping score.
 */
export interface Session {
  connected: boolean;
  error: string | null;
  project: ProjectView | null;
  capabilities: Capability[];
  timelines: TimelineSummary[];
  timeline: TimelineView | null;
  media: MediaView[];
  structure: StructureView | null;
  script: ScriptView | null;
  history: HistoryView | null;
  /** What the most recent change did — from this client or any other. */
  impact: ImpactView | null;
  /** Most recent commits, newest first — the shared history, not a local log. */
  changes: ChangeNotification[];
  /** The production office's derived state, when there is a screenplay. */
  production: ProductionView | null;
  /** The colour config, what is being looked through, and each file's colorspace. */
  colour: ColorView | null;
  /** Background work, newest first, live. */
  jobs: JobView[];
  /** The last refused request, in the runtime's words. */
  refusal: string | null;
  /** Which file every surface reads: originals, proxies, or per clip. */
  mediaPreference: MediaPreference;
  setMediaPreference(preference: MediaPreference): Promise<void>;
  /** Look for every original on disk now. */
  checkMedia(): Promise<void>;
  client: RpcClient;
  selectTimeline(id: string): void;
  setTrackState(track: string, key: TrackStateKey, value: boolean): Promise<void>;
  /** Run a mutating call; a refusal is shown, not thrown. */
  run<T>(label: string, call: () => Promise<T>): Promise<T | null>;
  undo(): Promise<void>;
  redo(): Promise<void>;
  save(): Promise<void>;
  refresh(): Promise<void>;
}

export function runtimeUrl(): string {
  const override = (import.meta as unknown as { env?: Record<string, string> }).env
    ?.VITE_CINE_BRIDGE;
  if (override) return override;
  if (typeof window !== "undefined" && window.location?.host) {
    const scheme = window.location.protocol === "https:" ? "wss" : "ws";
    return `${scheme}://${window.location.host}/ws`;
  }
  return "ws://127.0.0.1:7331/ws";
}

/**
 * One user action produces a burst of commits — the edit, then each reactor's
 * transaction. Refetching per commit redraws the application once per reactor.
 * Waiting this long after the last notification of a burst refetches once.
 */
export const SETTLE_MS = 24;

export function useSession(url: string = runtimeUrl()): Session {
  const client = useMemo(() => new RpcClient(url), [url]);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [project, setProject] = useState<ProjectView | null>(null);
  const [capabilities, setCapabilities] = useState<Capability[]>([]);
  const [timelines, setTimelines] = useState<TimelineSummary[]>([]);
  const [timeline, setTimeline] = useState<TimelineView | null>(null);
  const [media, setMedia] = useState<MediaView[]>([]);
  const [structure, setStructure] = useState<StructureView | null>(null);
  const [script, setScript] = useState<ScriptView | null>(null);
  const [history, setHistory] = useState<HistoryView | null>(null);
  const [impact, setImpact] = useState<ImpactView | null>(null);
  const [changes, setChanges] = useState<ChangeNotification[]>([]);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [jobs, setJobs] = useState<JobView[]>([]);
  const [mediaPreference, setMediaPreference] = useState<MediaPreference>("original");
  const [production, setProduction] = useState<ProductionView | null>(null);
  const [colour, setColour] = useState<ColorView | null>(null);
  const selected = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [p, list, m, s, sc, h] = await Promise.all([
        client.project(),
        client.timelines(),
        client.media(),
        client.structure(),
        client.script(),
        client.history(),
      ]);
      setProject(p);
      setTimelines(list);
      setMedia(m);
      setStructure(s);
      setScript(sc);
      setHistory(h);
      // No screenplay is a normal state, not an error.
      setProduction(sc.documents.length ? await client.production().catch(() => null) : null);
      // A project without a colour config is a normal state too.
      setColour(await client.color().catch(() => null));
      const known = list.some((t) => t.id === selected.current);
      const want = known ? selected.current : (list[0]?.id ?? null);
      selected.current = want;
      setTimeline(want ? await client.timeline(want) : null);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client]);

  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let latest = 0;
    client.onState((open) => {
      if (!live) return;
      setConnected(open);
      if (!open) setError("disconnected from the runtime");
    });
    // A commit — from this client or any other — is the only refresh trigger.
    client.onChange((n) => {
      if (!live) return;
      setChanges((c) => [n, ...c].slice(0, 200));
      latest = Math.max(latest, n.revision);
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        void refresh();
        void client
          .impact(latest)
          .then((i) => live && i && setImpact(i))
          .catch(() => {});
      }, SETTLE_MS);
    });
    // Jobs report progress many times a second; each notification is the whole
    // record, so it replaces what was there rather than being merged.
    client.onJob((job) => {
      if (!live) return;
      setJobs((all) => [job, ...all.filter((j) => j.id !== job.id)].sort((x, y) => y.id - x.id).slice(0, 200));
    });
    // Someone else chose what to play: follow it, as the native window does.
    client.onMediaPreference((p) => {
      if (!live) return;
      setMediaPreference(p);
      void refresh();
    });
    // Someone else chose a view: follow it, as the native window does.
    client.onView(() => {
      if (!live) return;
      void client.color().then((c) => live && setColour(c)).catch(() => {});
    });
    client.onFocus((timeline) => {
      if (!live || !timeline || timeline === selected.current) return;
      selected.current = timeline;
      void client.timeline(timeline).then((t) => live && setTimeline(t));
    });
    client
      .connect()
      .then(async () => {
        if (!live) return;
        setCapabilities(await client.capabilities());
        void client.jobs().then((j) => live && setJobs(j)).catch(() => {});
        void client
          .mediaPreference()
          .then((p) => live && setMediaPreference(p.preference))
          .catch(() => {});
        const focus = await client.focus();
        if (focus.timeline) selected.current = focus.timeline;
        await refresh();
        const last = await client.impact();
        if (live && last) setImpact(last);
      })
      .catch((e: Error) => {
        if (live) setError(e.message);
      });
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
    };
  }, [client, refresh]);

  // Opening a sequence opens it in every surface of the application, the
  // native window included; the announcement is what moves this one too.
  const selectTimeline = useCallback(
    (id: string) => {
      selected.current = id;
      void client.timeline(id).then(setTimeline);
      void client.focus(id).catch(() => {});
    },
    [client],
  );

  const run = useCallback(async <T,>(label: string, call: () => Promise<T>) => {
    try {
      const out = await call();
      setRefusal(null);
      return out;
    } catch (e) {
      setRefusal(`${label}: ${e instanceof Error ? e.message : String(e)}`);
      return null;
    }
  }, []);

  const setTrackState = useCallback(
    async (track: string, key: TrackStateKey, value: boolean) => {
      // No local write: the refresh that follows the broadcast is what changes
      // what is on screen, so this window learns the same way every other one
      // does.
      await run(`track ${key}`, () => client.trackState(track, key, value));
    },
    [client, run],
  );

  return {
    connected,
    error,
    project,
    capabilities,
    timelines,
    timeline,
    media,
    structure,
    script,
    history,
    impact,
    changes,
    refusal,
    mediaPreference,
    setMediaPreference: async (preference: MediaPreference) => {
      const answer = await run("play media", () => client.mediaPreference(preference));
      if (answer) setMediaPreference(answer.preference);
      await refresh();
    },
    checkMedia: async () => {
      await run("check media", () => client.checkMedia());
      await refresh();
    },
    production,
    colour,
    jobs,
    client,
    selectTimeline,
    setTrackState,
    run,
    undo: async () => void (await run("undo", () => client.undo())),
    redo: async () => void (await run("redo", () => client.redo())),
    save: async () => void (await run("save", () => client.save())),
    refresh,
  };
}
