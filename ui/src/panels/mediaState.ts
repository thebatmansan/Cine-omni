import type { MediaAvailability, MediaView } from "../rpc/client";

/**
 * How one media's state reads in the interface.
 *
 * Every surface asks this rather than testing a flag. Five of them used to
 * decide independently from `online !== false`, which cannot tell "never
 * established" from "established as present", so a take nobody had ever looked
 * at was shown as online at its recorded path.
 */
export type Tone = "ok" | "caution" | "bad";

export interface Reading {
  /** The state itself — two or three words, not a sentence. */
  label: string;
  /** How the project came to believe it, for the line beneath. */
  grounds: string;
  /** The sentence explaining it, where there is one. */
  detail: string | null;
  tone: Tone;
  /** The runtime operation that resolves it, if one does. */
  action: "check_media" | "relink" | null;
  actionLabel: string | null;
}

const TONE: Record<MediaAvailability["presence"], Tone> = {
  present: "ok",
  never_probed: "caution",
  offline: "bad",
  replaced: "bad",
  not_found_by_content: "bad",
  undecodable: "bad",
  not_on_this_machine: "bad",
  no_such_media: "bad",
};

const ACTION_LABEL: Record<NonNullable<MediaAvailability["action"]>, string> = {
  check_media: "Check media",
  relink: "Relink",
};

/** How long ago, in the shortest form that is still exact enough to act on. */
export function since(at: number | null): string {
  if (at === null) return "";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - at);
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}

export function read(state: MediaAvailability): Reading {
  const grounds =
    state.evidence === "checked"
      ? `Confirmed ${since(state.at)}`
      : state.evidence === "at_import"
        ? // The distinction M1 exists for: this is what the project recorded
          // when the media was imported, not what is true now.
          `Recorded at import ${since(state.at)}`
        : "Never established";
  return {
    label: state.label,
    grounds,
    detail: state.detail,
    tone: TONE[state.presence] ?? "bad",
    action: state.action,
    actionLabel: state.action ? ACTION_LABEL[state.action] : null,
  };
}

/**
 * The state of a media the interface holds, tolerating a runtime that predates
 * this field rather than inventing one.
 */
export function stateOf(media: MediaView): MediaAvailability | null {
  return media.state ?? null;
}
