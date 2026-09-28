import { useState } from "react";
import { Cdl, ColorView, GradeOp, GradeView, RpcClient, ViewSelection } from "../rpc/client";
import { useGrade } from "../state/grade";

/** A grade that changes nothing: what a clip has before anyone touches it. */
const NEUTRAL: Cdl = { slope: [1, 1, 1], offset: [0, 0, 0], power: [1, 1, 1], saturation: 1 };

/**
 * Colour management: the project's config, what everything is being looked at
 * through, and what each file is in.
 *
 * Two honesties matter here. Which picture is graded is not a detail: a clip's
 * picture is served through the project's view with that clip's grade applied,
 * and a file's picture is not, because the same file may back several clips
 * with different grades and showing one of them as the file's colour would be
 * a lie about whose grade it is. So the record monitor beside this panel is
 * where a grade is seen, and the source viewer stays the file. Real-time
 * playback is still native — a frame must not leave the GPU — and these are
 * still single decoded frames, not a running picture. And every file's
 * colorspace says who decided it: a file rule from the config, or a person.
 */
export function Colour({
  colour,
  client,
  revision,
  onView,
  onInputColorspace,
  onGrade,
  onGradeOps,
  onPickClip,
}: {
  colour: ColorView | null;
  client: RpcClient;
  /** Refetch the chosen clip's grade whenever the project changes. */
  revision: number;
  onView(selection: ViewSelection | null): void;
  onInputColorspace(media: string, colorspace: string): void;
  /** Grade a clip: the viewer rebakes and the picture changes. */
  onGrade?(clip: string, cdl: Cdl): void;
  /** Replace the operations of the clip's primary grade layer. */
  onGradeOps?(clip: string, ops: GradeOp[]): void;
  /** Put the playhead on this clip, so the record monitor shows it. */
  onPickClip?(clip: string): void;
}) {
  const [gradedClip, setGradedClip] = useState<string | null>(null);
  // The clip whose grade is being edited: the chosen one while it is still in
  // the sequence, otherwise the first. Resolved before the early return so the
  // grade fetch is not a conditional hook.
  const clips = colour?.clips ?? [];
  const chosen = clips.find((c) => c.clip === gradedClip)?.clip ?? clips[0]?.clip ?? null;
  const { grade, refusal: gradeRefusal } = useGrade(client, chosen, revision);
  if (!colour || !colour.config) {
    return (
      <div className="panel colour">
        <div className="panel-head">Colour</div>
        <div className="panel-body pad">
          <div className="empty">This project has no colour config.</div>
        </div>
      </div>
    );
  }
  const displays = colour.displays.filter((d) => d.active);
  const selected = colour.view;
  const currentDisplay = displays.find((d) => d.name === selected?.display) ?? displays[0] ?? null;
  const choices = colour.colorspaces.filter((c) => !c.isdata);

  return (
    <div className="panel colour">
      <div className="panel-head">
        Colour
        <span className="dim"> · {colour.config.name}</span>
      </div>
      <div className="panel-body pad">
        <div className="section-title">Viewing</div>
        <div className="binding">
          <select
            aria-label="Display"
            value={currentDisplay?.name ?? ""}
            onChange={(e) => {
              const display = colour.displays.find((d) => d.name === e.target.value);
              const view = display?.views.find((v) => v.active) ?? display?.views[0];
              if (display && view) onView({ display: display.name, view: view.name });
            }}
          >
            {displays.map((d) => (
              <option key={d.name} value={d.name}>
                {d.name}
              </option>
            ))}
          </select>
          <select
            aria-label="View"
            value={selected?.view ?? ""}
            onChange={(e) => {
              if (currentDisplay) onView({ display: currentDisplay.name, view: e.target.value });
            }}
          >
            {!selected && <option value="">(the config's default)</option>}
            {(currentDisplay?.views ?? []).map((v) => (
              <option key={v.name} value={v.name}>
                {v.name}
              </option>
            ))}
          </select>
          {selected && <button onClick={() => onView(null)}>Default</button>}
        </div>
        <div className="hint">
          The record monitor shows the clip under the playhead through this view, with that clip’s
          grade. The source viewer shows the file itself, ungraded, because one file can back
          several clips. Playback stays in the native window, where the picture stays on the GPU.
        </div>

        <div className="section-title">Media</div>
        {colour.media.length === 0 && <div className="hint">No media in this project yet.</div>}
        {colour.media.map((m) => (
          <div key={m.id} className="colour-row">
            <span className="label" title={m.path ?? undefined}>
              {m.name}
            </span>
            <select
              aria-label={`Input colorspace for ${m.name}`}
              value={m.input_colorspace ?? ""}
              onChange={(e) => onInputColorspace(m.id, e.target.value)}
            >
              {!m.input_colorspace && <option value="">(nothing established)</option>}
              {choices.map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name}
                </option>
              ))}
            </select>
            <span className={m.source === "user" ? "tag" : "dim"}>
              {m.source === "user" ? "chosen" : (m.source ?? "—").replace(/^rule:/, "rule ")}
            </span>
          </div>
        ))}

        <div className="section-title">Grade</div>
        {colour.clips.length === 0 && (
          <div className="hint">No sequence open, or it has no clips to grade.</div>
        )}
        {colour.clips.length > 0 && (
          <>
            <div className="chips">
              {colour.clips.map((c) => (
                <button
                  key={c.clip}
                  className={`chip${c.graded ? " tagged" : ""}`}
                  aria-selected={c.clip === chosen}
                  onClick={() => {
                    setGradedClip(c.clip);
                    // Choosing a clip to grade puts the playhead on it, so the
                    // monitor shows the shot being graded rather than whatever
                    // the playhead happened to be over.
                    onPickClip?.(c.clip);
                  }}
                  title={c.input_colorspace ?? "no colorspace established"}
                >
                  {c.name}
                  {c.graded && <span className="mark">•</span>}
                </button>
              ))}
            </div>
            <GradeControls
              clip={colour.clips.find((c) => c.clip === chosen) ?? colour.clips[0]}
              onGrade={onGrade}
            />
            <Stack
              clip={chosen}
              grade={grade}
              refusal={gradeRefusal}
              onOps={onGradeOps}
            />
          </>
        )}

        {colour.config.unimplemented.length > 0 && (
          <>
            <div className="section-title">Not applied from this config</div>
            <div className="hint">{colour.config.unimplemented.join(", ")}</div>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * ASC CDL for one clip: slope, offset, power per channel, then saturation.
 *
 * The numbers are the numbers — an ASC CDL is four values a colourist and a
 * facility both understand, and a slider that hides them would be a worse
 * control, not a friendlier one. What is missing is said: this is a primary
 * grade, with no wheels, curves, windows or keys behind it.
 */
function GradeControls({
  clip,
  onGrade,
}: {
  clip: { clip: string; name: string; cdl: Cdl | null };
  onGrade?(clip: string, cdl: Cdl): void;
}) {
  const cdl = clip.cdl ?? NEUTRAL;
  const change = (next: Cdl) => onGrade?.(clip.clip, next);
  const triplet = (label: string, key: "slope" | "offset" | "power", step: number) => (
    <div className="cdl-row">
      <span className="label">{label}</span>
      {([0, 1, 2] as const).map((i) => (
        <input
          key={i}
          type="number"
          step={step}
          value={cdl[key][i]}
          aria-label={`${label} ${"RGB"[i]}`}
          disabled={!onGrade}
          onChange={(e) => {
            const v = Number(e.target.value);
            if (!Number.isFinite(v)) return;
            const next = { ...cdl, [key]: [...cdl[key]] as [number, number, number] };
            next[key][i] = v;
            change(next);
          }}
        />
      ))}
    </div>
  );
  return (
    <div className="cdl">
      {triplet("slope", "slope", 0.05)}
      {triplet("offset", "offset", 0.01)}
      {triplet("power", "power", 0.05)}
      <div className="cdl-row">
        <span className="label">sat</span>
        <input
          type="number"
          step={0.05}
          min={0}
          value={cdl.saturation}
          aria-label="saturation"
          disabled={!onGrade}
          onChange={(e) => {
            const v = Number(e.target.value);
            if (Number.isFinite(v) && v >= 0) change({ ...cdl, saturation: v });
          }}
        />
        <button disabled={!onGrade} onClick={() => change(NEUTRAL)} title="Back to neutral">
          reset
        </button>
      </div>
      <div className="hint">
        The ASC CDL that came with the shot, applied in the working space before the grade stack
        below it. This is the value a facility exchanges; the grade is the work done on top.
      </div>
    </div>
  );
}

/**
 * What a control does when the operation it edits is not on the clip yet: the
 * value that changes nothing. Adding an operation must not change the picture
 * until somebody moves a number.
 */
const NEUTRAL_OP: Record<string, GradeOp> = {
  exposure: { op: "exposure", stops: 0 },
  // `out = (in − pivot) · amount + pivot`, so neutral is 1, not 0.
  contrast: { op: "contrast", amount: 1, pivot: 0.18 },
  saturation: { op: "saturation", amount: 1 },
  temperature_tint: { op: "temperature_tint", temperature_k: 0, tint: 0 },
  printer_lights: { op: "printer_lights", points: [0, 0, 0] },
};

/** The fields each supported operation has, and how far a click moves them. */
const FIELDS: Record<string, { key: string; label: string; step: number; min?: number }[]> = {
  exposure: [{ key: "stops", label: "stops", step: 0.1 }],
  contrast: [
    { key: "amount", label: "amount", step: 0.05 },
    { key: "pivot", label: "pivot", step: 0.01, min: 0.0001 },
  ],
  saturation: [{ key: "amount", label: "amount", step: 0.05, min: 0 }],
  temperature_tint: [
    { key: "temperature_k", label: "K shift", step: 100 },
    { key: "tint", label: "tint", step: 1 },
  ],
  printer_lights: [],
};

/**
 * The colourist's grade on one clip: the primary layer, as far as this protocol
 * carries it.
 *
 * Only the operations the runtime reports as supported are offered, and the
 * ones it does not carry are listed with the runtime's own reason — the engine
 * evaluates wheels, curves, LUTs, masks and qualifiers, and a control for them
 * that could not be driven would be worse than none. Nothing here is a local
 * model of a grade: every value shown came from `grade`, and every change is a
 * `grade_set` the runtime answers.
 *
 * A first layer this protocol cannot speak for — one holding a curve, or one
 * that is masked — disables the controls and says why, because writing a
 * primary into it would destroy work or confine it to part of the frame.
 */
function Stack({
  clip,
  grade,
  refusal,
  onOps,
}: {
  clip: string | null;
  grade: GradeView | null;
  refusal: string | null;
  onOps?(clip: string, ops: GradeOp[]): void;
}) {
  if (refusal) return <div className="hint">{refusal}</div>;
  if (!clip || !grade) return null;
  const ops = grade.layers[0]?.ops ?? [];
  const settable = grade.primary_settable && !!onOps;
  const write = (next: GradeOp[]) => onOps?.(clip, next);
  // Order is the grade: exposure before saturation is not the same picture as
  // saturation before exposure. So an operation already on the clip is
  // replaced where it stands, and a new one goes on the end.
  const set = (name: string, op: GradeOp | null) => {
    if (!op) return write(ops.filter((o) => o.op !== name));
    if (!ops.some((o) => o.op === name)) return write([...ops, op]);
    write(ops.map((o) => (o.op === name ? op : o)));
  };
  const number = (op: GradeOp, key: string) => (op as unknown as Record<string, number>)[key];

  return (
    <div className="stack">
      <div className="section-title sub">
        Grade stack
        {grade.layers.length > 1 && (
          <span className="dim"> · {grade.layers.length} layers, showing the first</span>
        )}
      </div>
      {!grade.primary_settable && grade.primary_why_not && (
        <div className="hint warn">{grade.primary_why_not}</div>
      )}
      {/* An operation the runtime carries that this build has no control for
          is named rather than drawn: a checkbox with no fields behind it would
          write a value nobody chose. */}
      {grade.supported.some((n) => !(n in NEUTRAL_OP)) && (
        <div className="hint">
          This protocol also carries {grade.supported.filter((n) => !(n in NEUTRAL_OP)).join(", ")},
          which this interface has no control for yet.
        </div>
      )}
      {grade.supported.filter((name) => name in NEUTRAL_OP).map((name) => {
        const on = ops.find((o) => o.op === name) ?? null;
        return (
          <div key={name} className="op-row">
            <label className="label">
              <input
                type="checkbox"
                checked={!!on}
                aria-label={name}
                disabled={!settable}
                onChange={(e) => set(name, e.target.checked ? NEUTRAL_OP[name] : null)}
              />
              {name.replace(/_/g, " ")}
            </label>
            {name === "printer_lights"
              ? ([0, 1, 2] as const).map((i) => (
                  <input
                    key={i}
                    type="number"
                    step={1}
                    aria-label={`printer lights ${"RGB"[i]}`}
                    disabled={!settable || !on}
                    value={on && "points" in on ? on.points[i] : 0}
                    onChange={(e) => {
                      const v = Number(e.target.value);
                      if (!Number.isFinite(v) || !on || !("points" in on)) return;
                      const points = [...on.points] as [number, number, number];
                      points[i] = v;
                      set(name, { op: "printer_lights", points });
                    }}
                  />
                ))
              : (FIELDS[name] ?? []).map((f) => (
                  <label key={f.key} className="field">
                    <span className="dim">{f.label}</span>
                    <input
                      type="number"
                      step={f.step}
                      min={f.min}
                      aria-label={`${name} ${f.label}`}
                      disabled={!settable || !on}
                      value={on ? number(on, f.key) : number(NEUTRAL_OP[name], f.key)}
                      onChange={(e) => {
                        const v = Number(e.target.value);
                        if (!Number.isFinite(v) || !on) return;
                        if (f.min !== undefined && v < f.min) return;
                        set(name, { ...on, [f.key]: v } as GradeOp);
                      }}
                    />
                  </label>
                ))}
          </div>
        );
      })}
      {ops.length > 0 && (
        <div className="op-row">
          <button
            disabled={!settable}
            onClick={() => write([])}
            title="Remove every operation from the primary layer. The CDL above is its own value and stays."
          >
            clear stack
          </button>
          <span className="dim">
            {ops.length} operation{ops.length === 1 ? "" : "s"} on this clip
          </span>
        </div>
      )}
      <div className="section-title sub">Not on this protocol</div>
      <div className="hint">
        {grade.unsupported.map((u) => `${u.op} — ${u.reason}`).join("; ")}.
      </div>
    </div>
  );
}
