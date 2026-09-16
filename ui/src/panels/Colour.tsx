import { useState } from "react";
import { Cdl, ColorView, ViewSelection } from "../rpc/client";

/** A grade that changes nothing: what a clip has before anyone touches it. */
const NEUTRAL: Cdl = { slope: [1, 1, 1], offset: [0, 0, 0], power: [1, 1, 1], saturation: 1 };

/**
 * Colour management: the project's config, what everything is being looked at
 * through, and what each file is in.
 *
 * Two honesties matter here. Choosing a view changes what the *native window*
 * shows — the browser cannot receive GPU textures, so the pictures in this tab
 * are decoded frames with no transform applied, and it says so rather than
 * letting a chosen view imply otherwise. And every file's colorspace says who
 * decided it: a file rule from the config, or a person.
 */
export function Colour({
  colour,
  onView,
  onInputColorspace,
  onGrade,
}: {
  colour: ColorView | null;
  onView(selection: ViewSelection | null): void;
  onInputColorspace(media: string, colorspace: string): void;
  /** Grade a clip: the viewer rebakes and the picture changes. */
  onGrade?(clip: string, cdl: Cdl): void;
}) {
  const [gradedClip, setGradedClip] = useState<string | null>(null);
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
          The view is applied in the native window, where the picture stays on the GPU. The
          pictures in this browser are decoded frames with no transform applied.
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
                  aria-selected={c.clip === (gradedClip ?? colour.clips[0].clip)}
                  onClick={() => setGradedClip(c.clip)}
                  title={c.input_colorspace ?? "no colorspace established"}
                >
                  {c.name}
                  {c.graded && <span className="mark">•</span>}
                </button>
              ))}
            </div>
            <GradeControls
              clip={
                colour.clips.find((c) => c.clip === (gradedClip ?? colour.clips[0].clip)) ??
                colour.clips[0]
              }
              onGrade={onGrade}
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
        A primary grade, applied in the working space and baked into the view the native window
        shows. No wheels, curves, qualifiers or windows.
      </div>
    </div>
  );
}
