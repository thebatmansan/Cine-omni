import { Capability, CapabilityState } from "../rpc/client";

/**
 * The workspaces a filmmaking application is eventually made of.
 *
 * All of them are listed, and each says truthfully where it stands. Listing
 * only the finished ones would hide the shape of the application; pretending
 * the unfinished ones work would be worse. A workspace whose state is not
 * `verified` or `degraded` cannot be opened, and the tab says why.
 */
export interface WorkspaceDef {
  id: string;
  name: string;
  /** The capability that decides whether this workspace can be entered. */
  gate?: string;
  /** Said when there is nothing behind it yet. */
  pending?: string;
}

export const WORKSPACES: WorkspaceDef[] = [
  { id: "editorial", name: "Editorial", gate: "editorial.timeline" },
  { id: "media", name: "Media", gate: "media.preview" },
  { id: "script", name: "Script", gate: "script.lines" },
  { id: "storyboard", name: "Storyboard", pending: "Board frames exist in the schema; nothing is built." },
  { id: "previs", name: "Shot / Previs", pending: "Previs cameras and stages are in the graph; no viewport exists." },
  { id: "animation", name: "3D / Animation", pending: "Not started. No 3D workspace exists." },
  { id: "vfx", name: "VFX / Compositing", pending: "A node graph exists in cine-comp; no interface is built." },
  { id: "colour", name: "Colour", gate: "colour.management" },
  { id: "audio", name: "Audio", gate: "audio.monitoring" },
  { id: "production", name: "Production", pending: "Shot lists, schedules and call sheets exist in the graph; no interface is built." },
  { id: "delivery", name: "Delivery", pending: "Not started. Export exists on the command line only." },
];

export function workspaceState(def: WorkspaceDef, capabilities: Capability[]): CapabilityState {
  if (!def.gate) return "unavailable";
  return capabilities.find((c) => c.id === def.gate)?.state ?? "unavailable";
}

export function workspaceReason(def: WorkspaceDef, capabilities: Capability[]): string {
  if (def.pending) return def.pending;
  return capabilities.find((c) => c.id === def.gate)?.detail ?? "Not available in this build.";
}

export function canOpen(def: WorkspaceDef, capabilities: Capability[]): boolean {
  const state = workspaceState(def, capabilities);
  return state === "verified" || state === "degraded";
}
