import { describe, expect, it } from "vitest";
import { eighths, stripClass } from "./Production";

describe("stripboard conventions", () => {
  it("writes pages in eighths the way a script supervisor does", () => {
    expect(eighths(3)).toBe("3/8");
    expect(eighths(8)).toBe("1");
    expect(eighths(11)).toBe("1 3/8");
  });

  it("colours strips by interior/exterior and day/night", () => {
    const s = (int_ext: string, time_of_day: string) =>
      stripClass({ scene: "", number: "", pages_eighths: 0, int_ext, location: "", time_of_day, cast: [], est_minutes: 0 });
    expect(s("INT", "DAY")).toBe("int-day");
    expect(s("EXT", "NIGHT")).toBe("ext-night");
    expect(s("Ext", "Dusk")).toBe("ext-night");
    expect(s("INT", "DAWN")).toBe("int-day");
  });
});
