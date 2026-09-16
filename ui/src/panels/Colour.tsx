import { ColorView, ViewSelection } from "../rpc/client";

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
}: {
  colour: ColorView | null;
  onView(selection: ViewSelection | null): void;
  onInputColorspace(media: string, colorspace: string): void;
}) {
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
