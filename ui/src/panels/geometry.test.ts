import { describe, expect, it } from "vitest";
import { clockLabel, tickPositions, tickStep } from "./geometry";

describe("timeline geometry", () => {
  it("picks a tick spacing that leaves room for the label", () => {
    // Zoomed out: one tick a minute, not one a second.
    expect(tickStep(1)).toBeGreaterThanOrEqual(60);
    // Zoomed in far enough that sub-second ticks fit.
    expect(tickStep(2000)).toBeLessThanOrEqual(0.2);
    // Every spacing gives at least the requested room.
    for (const zoom of [0.5, 4, 25, 90, 400, 3000]) {
      expect(tickStep(zoom, 64) * zoom).toBeGreaterThanOrEqual(64);
    }
  });

  it("covers the whole duration and is bounded however absurd the zoom", () => {
    const ticks = tickPositions(14, 90);
    expect(ticks[0].seconds).toBe(0);
    expect(ticks[ticks.length - 1].seconds).toBeGreaterThanOrEqual(14);

    // A degenerate zoom must fail to draw rather than fail to return: the same
    // class of bug hung the native workspace on an unbounded rectangle.
    const absurd = tickPositions(1e9, 4000);
    expect(absurd.length).toBeLessThanOrEqual(2000);
  });

  it("labels coarse ticks as clock time and fine ticks with hundredths", () => {
    expect(clockLabel(0, 1)).toBe("0:00");
    expect(clockLabel(75, 5)).toBe("1:15");
    expect(clockLabel(3725, 60)).toBe("1:02:05");
    expect(clockLabel(1.25, 0.2)).toBe("0:01.25");
  });
});
