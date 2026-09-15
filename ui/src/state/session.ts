import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Capability,
  ChangeNotification,
  MediaView,
  ProjectView,
  RpcClient,
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
  /** Most recent commits, newest first — the shared history, not a local log. */
  changes: ChangeNotification[];
  client: RpcClient;
  selectTimeline(id: string): void;
  setTrackState(track: string, key: TrackStateKey, value: boolean): Promise<void>;
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
  const [changes, setChanges] = useState<ChangeNotification[]>([]);
  const selected = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [p, list, m, s] = await Promise.all([
        client.project(),
        client.timelines(),
        client.media(),
        client.structure(),
      ]);
      setProject(p);
      setTimelines(list);
      setMedia(m);
      setStructure(s);
      const want = selected.current ?? list[0]?.id ?? null;
      selected.current = want;
      setTimeline(want ? await client.timeline(want) : null);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client]);

  useEffect(() => {
    let live = true;
    client.onState((open) => {
      if (!live) return;
      setConnected(open);
      if (!open) setError("disconnected from the runtime");
    });
    // A commit — from this client or any other — is the only refresh trigger.
    client.onChange((n) => {
      if (!live) return;
      setChanges((c) => [n, ...c].slice(0, 200));
      void refresh();
    });
    client
      .connect()
      .then(async () => {
        if (!live) return;
        setCapabilities(await client.capabilities());
        await refresh();
      })
      .catch((e: Error) => {
        if (live) setError(e.message);
      });
    return () => {
      live = false;
    };
  }, [client, refresh]);

  const selectTimeline = useCallback(
    (id: string) => {
      selected.current = id;
      void client.timeline(id).then(setTimeline);
    },
    [client],
  );

  const setTrackState = useCallback(
    async (track: string, key: TrackStateKey, value: boolean) => {
      // No local write: the refresh that follows the broadcast is what changes
      // what is on screen, so this window learns the same way every other one
      // does.
      await client.trackState(track, key, value);
    },
    [client],
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
    changes,
    client,
    selectTimeline,
    setTrackState,
    undo: async () => void (await client.undo()),
    redo: async () => void (await client.redo()),
    save: async () => void (await client.save()),
    refresh,
  };
}
