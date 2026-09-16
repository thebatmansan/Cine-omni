import { useEffect, useMemo, useState } from "react";
import type {
  CallSheetView,
  LDate,
  LTime,
  ProductionView,
  SceneBreakdown,
  ScheduleConstraints,
  Strip,
} from "../rpc/client";

/**
 * The production office: the stripboard, the constraints it was built under,
 * each day's call sheet, and the selected scene's breakdown and shots.
 *
 * Everything shown is the logistics reactor's current answer for the
 * screenplay. The one intent here is changing the constraints: the runtime
 * reschedules, and the board, the call sheets and the impact panel all update
 * from the broadcast.
 */
/** What each Day Out of Days code means, said where it is shown. */
const CODE_MEANING: Record<string, string> = {
  SW: "Start Work — first day, and they work it",
  W: "Work",
  H: "Hold — not shooting, not released, still paid",
  WF: "Work Finish — last day they work",
  SWF: "Start Work Finish — starts and finishes the same day",
};

export function Production({
  production,
  onConstraints,
  onMoveStrip,
  onTag,
}: {
  production: ProductionView | null;
  onConstraints(document: string, c: ScheduleConstraints): void;
  /** Put a scene on a day, or `null` to release it back to the scheduler. */
  onMoveStrip?(document: string, scene: string, day: number | null): void;
  /** Tag or untag a breakdown element; `present` undefined reverts to the script. */
  onTag?(scene: string, category: string, value: string, present?: boolean): void;
}) {
  const [scene, setScene] = useState<string | null>(null);
  const [day, setDay] = useState<number>(1);
  if (!production) {
    return (
      <div className="panel">
        <div className="panel-head">Production</div>
        <div className="empty">No screenplay in this project. Import one from Project → Import screenplay.</div>
      </div>
    );
  }
  const days = production.schedule?.days ?? [];
  const breakdown = production.breakdowns.find((b) => b.scene === scene) ?? production.breakdowns[0] ?? null;
  const sheet = production.call_sheets.find((c) => c.day === day) ?? production.call_sheets[0] ?? null;
  const shots = production.shots.find((s) => s.scene === breakdown?.scene)?.shots ?? [];

  return (
    <>
      <div className="panel constraints-panel">
        <div className="panel-head">Schedule</div>
        <div className="panel-body pad">
          <Constraints
            key={production.document + JSON.stringify(production.constraints)}
            value={production.constraints}
            onApply={(c) => onConstraints(production.document, c)}
          />
          <div className="section-title">Summary</div>
          <dl className="props">
            <dt>shoot days</dt>
            <dd>{days.length}</dd>
            <dt>scenes</dt>
            <dd>{production.breakdowns.length}</dd>
            <dt>pages</dt>
            <dd>{eighths(production.breakdowns.reduce((a, b) => a + b.page_eighths, 0))}</dd>
            <dt>company moves</dt>
            <dd>{production.schedule?.company_moves ?? "—"}</dd>
          </dl>
          {(production.schedule?.warnings.length ?? 0) > 0 && (
            <div className="notice">
              <strong>The scheduler had to break a constraint</strong>
              {production.schedule!.warnings.map((w) => (
                <div key={w}>{w}</div>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="panel dood-panel">
        <div className="panel-head">
          <span>Day Out of Days</span>
          <span className="spacer" />
          <span className="legend">
            {production.dood && (
              <span className="dim">
                {production.dood.total_worked} worked · {production.dood.total_held} held
              </span>
            )}
          </span>
        </div>
        <div className="panel-body dood">
          {!production.dood && <div className="empty">No schedule yet.</div>}
          {production.dood && (
            <table>
              <thead>
                <tr>
                  <th>Cast</th>
                  {production.dood.days.map((d) => (
                    <th key={d}>{d}</th>
                  ))}
                  <th>W</th>
                  <th>H</th>
                </tr>
              </thead>
              <tbody>
                {production.dood.cast.map((row) => (
                  <tr key={row.name}>
                    <td className="who">{row.name}</td>
                    {row.days.map((code, i) => (
                      <td
                        key={production.dood!.days[i]}
                        className={`code ${code ? code.toLowerCase() : "off"}`}
                        title={CODE_MEANING[code] ?? "not engaged"}
                      >
                        {code}
                      </td>
                    ))}
                    <td>{row.worked}</td>
                    <td className={row.held > 0 ? "held" : ""}>{row.held}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <div className="hint">
            A hold day is a day an actor is not shooting and not released — still paid. Re-ordering
            the board changes it; the work does not.
          </div>
        </div>
      </div>

      <div className="panel stripboard-panel">
        <div className="panel-head">
          <span>Stripboard</span>
          <span className="spacer" />
          <span className="legend">
            <span className="strip-key int-day">INT day</span>
            <span className="strip-key ext-day">EXT day</span>
            <span className="strip-key int-night">INT night</span>
            <span className="strip-key ext-night">EXT night</span>
          </span>
        </div>
        <div className="panel-body stripboard">
          {days.length === 0 && <div className="empty">No schedule yet.</div>}
          {days.map((d) => (
            <div key={d.index} className={`board-day${d.index === (sheet?.day ?? 0) ? " selected" : ""}`}>
              <button className="day-banner" onClick={() => setDay(d.index)}>
                <span>Day {d.index}</span>
                <span className="meta">{dateLabel(d.date)}</span>
                <span className="meta">{eighths(d.total_eighths)} pp</span>
              </button>
              {d.strips.map((s) => {
                const pinnedTo = production.constraints.pinned?.[s.scene] ?? null;
                return (
                  <div key={s.scene} className={`strip-row${pinnedTo ? " pinned" : ""}`}>
                    <button
                      className={`strip ${stripClass(s)}`}
                      aria-selected={s.scene === breakdown?.scene}
                      onClick={() => {
                        setScene(s.scene);
                        setDay(d.index);
                      }}
                      title={`${s.int_ext} ${s.location} — ${s.time_of_day}${
                        pinnedTo ? ` — put on day ${pinnedTo} by hand` : ""
                      }`}
                    >
                      <span className="num">{s.number}</span>
                      <span className="where">
                        {s.int_ext} {s.location}
                      </span>
                      <span className="tod">{s.time_of_day}</span>
                      <span className="pages">{eighths(s.pages_eighths)}</span>
                      <span className="cast">{s.cast.join(", ")}</span>
                      {pinnedTo && <span className="pin" title="pinned by hand">📌</span>}
                    </button>
                    {onMoveStrip && (
                      <span className="strip-move">
                        <button
                          aria-label={`Move scene ${s.number} one day earlier`}
                          disabled={d.index <= 1}
                          onClick={() => onMoveStrip(production.document, s.scene, d.index - 1)}
                        >
                          ↑
                        </button>
                        <button
                          aria-label={`Move scene ${s.number} one day later`}
                          onClick={() => onMoveStrip(production.document, s.scene, d.index + 1)}
                        >
                          ↓
                        </button>
                        <button
                          aria-label={`Release scene ${s.number} to the scheduler`}
                          disabled={!pinnedTo}
                          title="Let the scheduler decide again"
                          onClick={() => onMoveStrip(production.document, s.scene, null)}
                        >
                          ✕
                        </button>
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </div>

      <div className="right-stack">
        <div className="panel">
          <div className="panel-head">
            <span>Call sheet</span>
            {sheet && <span className="count">Day {sheet.day} · {dateLabel(sheet.date)}</span>}
            <span className="spacer" />
            {sheet && (
              <a
                className="pdf-link"
                href={`/callsheet?day=${sheet.day}`}
                target="_blank"
                rel="noreferrer"
                title="The printable call sheet for this day"
              >
                PDF
              </a>
            )}
          </div>
          <div className="panel-body pad">{sheet ? <Sheet sheet={sheet} /> : <div className="empty">—</div>}</div>
        </div>
        <div className="panel">
          <div className="panel-head">
            <span>Breakdown</span>
            {breakdown && <span className="count">Scene {breakdown.number}</span>}
          </div>
          <div className="panel-body pad">
            {breakdown && (
              <>
                <div className="bd-heading">{breakdown.heading}</div>
                <dl className="props">
                  <dt>pages</dt>
                  <dd>{eighths(breakdown.page_eighths)}</dd>
                  <dt>est. screen time</dt>
                  <dd>{breakdown.est_minutes.toFixed(1)} min</dd>
                  <dt>extras</dt>
                  <dd>{breakdown.extras_hint ?? "—"}</dd>
                </dl>
                <Tagged
                  label="cast"
                  category="cast"
                  breakdown={breakdown}
                  items={breakdown.cast}
                  added={breakdown.tags?.cast_added ?? []}
                  onTag={onTag}
                />
                <Tagged
                  label="props"
                  category="prop"
                  breakdown={breakdown}
                  items={breakdown.props}
                  added={breakdown.tags?.props_added ?? []}
                  onTag={onTag}
                />
                <div className="section-title">Flags</div>
                <div className="flags">
                  {([
                    ["vehicles", breakdown.vehicles, breakdown.tags?.vehicles],
                    ["animals", breakdown.animals, breakdown.tags?.animals],
                    ["sfx", breakdown.sfx, breakdown.tags?.sfx],
                    ["vfx", breakdown.vfx, breakdown.tags?.vfx],
                  ] as [string, boolean, boolean | null | undefined][]).map(([name, on, override]) => (
                    <label key={name} className={override != null ? "tagged" : ""}>
                      <input
                        type="checkbox"
                        checked={on}
                        disabled={!onTag}
                        onChange={(e) => onTag?.(breakdown.scene, name, "", e.target.checked)}
                      />
                      {name}
                      {override != null && (
                        <button
                          className="revert"
                          title="Let the script decide again"
                          onClick={() => onTag?.(breakdown.scene, name, "", undefined)}
                        >
                          ↺
                        </button>
                      )}
                    </label>
                  ))}
                </div>
                <div className="section-title">Shots</div>
                {shots.length === 0 && <div className="hint">No shots: this scene has no previs cameras yet.</div>}
                {shots.map((sh) => (
                  <div key={sh.id} className={`binding${sh.stale ? " conform" : ""}`}>
                    <span className="label">
                      {sh.number} · {sh.size} · {sh.lens_mm} mm · {sh.movement}
                    </span>
                    <span className="meta">{sh.takes.length} take{sh.takes.length === 1 ? "" : "s"}</span>
                    {sh.stale && <div className="reason">retired: its camera is gone, but takes were shot against it</div>}
                  </div>
                ))}
              </>
            )}
          </div>
        </div>
      </div>
    </>
  );
}

function Constraints({ value, onApply }: { value: ScheduleConstraints; onApply(c: ScheduleConstraints): void }) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const changed = useMemo(() => JSON.stringify(draft) !== JSON.stringify(value), [draft, value]);
  const num = (k: "max_pages_per_day" | "max_hours_per_day" | "shooting_days_per_week", step: number) => (
    <input
      type="number"
      step={step}
      value={draft[k]}
      onChange={(e) => setDraft({ ...draft, [k]: Number(e.target.value) })}
    />
  );
  return (
    <div className="constraints">
      <label>
        Pages per day {num("max_pages_per_day", 0.125)}
      </label>
      <label>
        Hours per day {num("max_hours_per_day", 0.5)}
      </label>
      <label>
        Shooting days per week {num("shooting_days_per_week", 1)}
      </label>
      <label>
        Start date
        <input
          type="date"
          value={isoDate(draft.start_date)}
          onChange={(e) => {
            const [y, m, d] = e.target.value.split("-").map(Number);
            if (y && m && d) setDraft({ ...draft, start_date: { year: y, month: m, day: d } });
          }}
        />
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={draft.group_by_location}
          onChange={(e) => setDraft({ ...draft, group_by_location: e.target.checked })}
        />
        Group by location
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={draft.night_shoots_together}
          onChange={(e) => setDraft({ ...draft, night_shoots_together: e.target.checked })}
        />
        Keep night and day apart
      </label>
      <div className="binding">
        <button className="primary" disabled={!changed} onClick={() => onApply(draft)}>
          Reschedule
        </button>
        <button disabled={!changed} onClick={() => setDraft(value)}>
          Revert
        </button>
      </div>
    </div>
  );
}

function Sheet({ sheet }: { sheet: CallSheetView }) {
  return (
    <>
      <dl className="props">
        <dt>general call</dt>
        <dd>{timeLabel(sheet.general_call)}</dd>
        <dt>locations</dt>
        <dd>{sheet.locations.join(" → ")}</dd>
      </dl>
      <div className="section-title">Scenes</div>
      <table className="sheet-table">
        <tbody>
          {sheet.scenes.map((s) => (
            <tr key={s.scene}>
              <td>{s.number}</td>
              <td>{s.heading}</td>
              <td>{s.pages}</td>
              <td>{s.cast_numbers.join(",")}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="section-title">Cast</div>
      <table className="sheet-table">
        <thead>
          <tr>
            <th>#</th>
            <th>name</th>
            <th>pickup</th>
            <th>makeup</th>
            <th>on set</th>
          </tr>
        </thead>
        <tbody>
          {sheet.cast_calls.map((c) => (
            <tr key={c.name}>
              <td>{c.number}</td>
              <td>{c.name}</td>
              <td>{timeLabel(c.pickup)}</td>
              <td>{timeLabel(c.makeup)}</td>
              <td>{timeLabel(c.on_set)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="section-title">Crew</div>
      <dl className="props">
        {sheet.crew_calls.map((c) => (
          <FragmentRow key={c.department} k={c.department} v={timeLabel(c.call)} />
        ))}
      </dl>
    </>
  );
}

function FragmentRow({ k, v }: { k: string; v: string }) {
  return (
    <>
      <dt>{k}</dt>
      <dd>{v}</dd>
    </>
  );
}

/** Industry strip colours: white INT day, yellow EXT day, blue INT night, green EXT night. */
export function stripClass(s: Strip): string {
  const night = /NIGHT|EVENING|DUSK/i.test(s.time_of_day);
  const ext = /EXT/i.test(s.int_ext);
  return `${ext ? "ext" : "int"}-${night ? "night" : "day"}`;
}

/** Eighths of a page as a script supervisor writes them: `1 3/8`. */
export function eighths(n: number): string {
  const whole = Math.floor(n / 8);
  const rest = n % 8;
  if (whole === 0) return `${rest}/8`;
  return rest === 0 ? `${whole}` : `${whole} ${rest}/8`;
}

const pad = (n: number) => String(n).padStart(2, "0");
const isoDate = (d: LDate) => `${d.year}-${pad(d.month)}-${pad(d.day)}`;
const dateLabel = (d: LDate) => isoDate(d);
const timeLabel = (t: LTime) => `${pad(t.hour)}:${pad(t.minute)}`;

/**
 * One breakdown list, showing what the script said and what a person added.
 *
 * A tag is marked, because the difference matters on the day: the script is
 * evidence, a tag is somebody's judgement, and the department reading it is
 * entitled to know which it is looking at.
 */
function Tagged({
  label,
  category,
  breakdown,
  items,
  added,
  onTag,
}: {
  label: string;
  category: string;
  breakdown: SceneBreakdown;
  items: string[];
  added: string[];
  onTag?(scene: string, category: string, value: string, present?: boolean): void;
}) {
  const [draft, setDraft] = useState("");
  const isTagged = (v: string) => added.some((a) => a.toLowerCase() === v.toLowerCase());
  return (
    <>
      <div className="section-title">{label}</div>
      <div className="chips">
        {items.length === 0 && <span className="hint">—</span>}
        {items.map((item) => (
          <span key={item} className={`chip${isTagged(item) ? " tagged" : ""}`}>
            {item}
            {isTagged(item) && <span className="mark" title="tagged by hand">•</span>}
            {onTag && (
              <button
                aria-label={`Remove ${item} from ${label}`}
                title={isTagged(item) ? "Remove this tag" : "Say this is not in the scene"}
                onClick={() => onTag(breakdown.scene, category, item, false)}
              >
                ×
              </button>
            )}
          </span>
        ))}
      </div>
      {onTag && (
        <form
          className="chip-add"
          onSubmit={(e) => {
            e.preventDefault();
            const value = draft.trim();
            if (!value) return;
            onTag(breakdown.scene, category, value, true);
            setDraft("");
          }}
        >
          <input
            value={draft}
            placeholder={`add ${label}…`}
            aria-label={`Add to ${label}`}
            onChange={(e) => setDraft(e.target.value)}
          />
          <button type="submit" disabled={!draft.trim()}>
            +
          </button>
        </form>
      )}
    </>
  );
}
