import { describe, expect, it } from "vitest";
import { Capability } from "../rpc/client";
import { WORKSPACES, canOpen, workspaceReason, workspaceState } from "./index";

const caps: Capability[] = [
  { id: "editorial.timeline", name: "Timeline", group: "Editorial", state: "verified", detail: "reads the graph" },
  { id: "media.preview", name: "Preview", group: "Media", state: "degraded", detail: "not real-time" },
  { id: "audio.monitoring", name: "Audio", group: "Audio", state: "unavailable", detail: "no device on this build" },
  { id: "colour.management", name: "Colour", group: "Colour", state: "unavailable", detail: "not ACES-verified" },
];

describe("workspace gating", () => {
  it("opens only what the runtime says is backed", () => {
    const state = (id: string) => {
      const def = WORKSPACES.find((w) => w.id === id)!;
      return { state: workspaceState(def, caps), open: canOpen(def, caps) };
    };
    expect(state("editorial")).toEqual({ state: "verified", open: true });
    // Degraded is still usable — it is honest, not broken.
    expect(state("media")).toEqual({ state: "degraded", open: true });
    expect(state("audio").open).toBe(false);
    expect(state("colour").open).toBe(false);
  });

  it("refuses to open a workspace with nothing behind it", () => {
    for (const id of ["animation", "delivery", "vfx", "storyboard", "previs", "production", "script"]) {
      const def = WORKSPACES.find((w) => w.id === id)!;
      expect(canOpen(def, caps)).toBe(false);
    }
  });

  it("opens the script workspace only when the runtime reports script lines", () => {
    const def = WORKSPACES.find((w) => w.id === "script")!;
    expect(canOpen(def, caps)).toBe(false);
    const withScript: Capability[] = [
      ...caps,
      { id: "script.lines", name: "Script", group: "Script", state: "degraded", detail: "lines only" },
    ];
    expect(canOpen(def, withScript)).toBe(true);
  });

  it("gives a reason for everything it will not open", () => {
    for (const def of WORKSPACES) {
      if (canOpen(def, caps)) continue;
      const why = workspaceReason(def, caps);
      expect(why.length).toBeGreaterThan(15);
    }
  });

  it("lists the whole intended application, not only what is finished", () => {
    // Hiding the unbuilt workspaces would hide the shape of the application.
    const ids = WORKSPACES.map((w) => w.id);
    for (const expected of ["editorial", "media", "script", "vfx", "colour", "audio", "delivery"]) {
      expect(ids).toContain(expected);
    }
  });

  it("never claims a capability the runtime did not report", () => {
    const def = WORKSPACES.find((w) => w.id === "editorial")!;
    expect(workspaceState(def, [])).toBe("unavailable");
    expect(canOpen(def, [])).toBe(false);
  });
});
