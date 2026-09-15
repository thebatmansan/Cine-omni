/**
 * Timeline geometry: turning times into pixels.
 *
 * Separated from the component so it can be tested without a DOM. These are the
 * only calculations the browser performs on time, and they exist purely to
 * place things on screen — no duration is ever summed here, because composition
 * is the runtime's answer and recomputing it would be a second one.
 */

export interface Tick {
  seconds: number;
  label: string;
}

/** Candidate spacings, coarse enough that labels never collide. */
const STEPS = [0.04, 0.2, 1, 2, 5, 10, 30, 60, 300, 600, 1800, 3600];

/** The tick spacing that gives each label at least `minPx` of room. */
export function tickStep(pxPerSecond: number, minPx = 64): number {
  return STEPS.find((s) => s * pxPerSecond >= minPx) ?? STEPS[STEPS.length - 1];
}

export function tickPositions(duration: number, pxPerSecond: number): Tick[] {
  const step = tickStep(pxPerSecond);
  const out: Tick[] = [];
  // Bounded on purpose: a degenerate zoom must fail to draw rather than fail to
  // return. The same class of bug once hung the native workspace.
  for (let t = 0; t <= duration + step && out.length < 2000; t += step) {
    out.push({ seconds: t, label: clockLabel(t, step) });
  }
  return out;
}

export function clockLabel(seconds: number, step: number): string {
  const whole = Math.floor(seconds);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const base = h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
  if (step >= 1) return base;
  return `${base}.${pad(Math.round((seconds - whole) * 100))}`;
}

const pad = (n: number) => n.toString().padStart(2, "0");
