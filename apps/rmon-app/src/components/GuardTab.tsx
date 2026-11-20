import { memo, useState } from "react";
import { backend } from "../api";
import { cwdShort, errText, fmtBytes, fmtClock, fmtDateTime, fmtSecs } from "../format";
import type { Author, GuardEvent, GuardMode, GuardSettings, GuardZone, Origin } from "../types";
import { guardStore, useGuard } from "../guardStore";
import { Switch } from "./Controls";
import { GuardSettingsPanel } from "./GuardSettingsPanel";
import { OrphanCleanupCard } from "./OrphanCleanupCard";

export const MODE_LABEL: Record<GuardMode["kind"], string> = {
  off: "выключен",
  armed: "боевой",
  dry_run: "проба",
  standby: "ждёт",
  broken: "стоит",
};

const MODE_HINT: Record<GuardMode["kind"], string> = {
  off: "Сторож ничего не снимает и не следит за зоной.",
  armed: "В красной зоне сам снимает самый большой прогон.",
  dry_run: "Следит и пишет, кого снял бы, но никого не трогает.",
  standby: "",
  broken: "",
};

const ZONE_LABEL: Record<GuardZone, string> = { green: "зелёная", yellow: "жёлтая", red: "красная" };

const OUTCOME_LABEL: Record<string, string> = {
  term: "завершился по SIGTERM",
  kill: "добит SIGKILL",
  alive: "не завершился",
  gone: "уже завершился сам",
  reused: "pid занят другим процессом — не тронут",
  denied: "нет прав",
  refused: "отказ защитного слоя",
};

function fmtWhen(ts: number): string {
  return Date.now() / 1000 - ts < 12 * 3600 ? fmtClock(ts) : fmtDateTime(ts);
}

function originText(o: Origin): string {
  switch (o.kind) {
    case "claude_code":
      return `Claude Code${o.title ? ` «${o.title}»` : ""}`;
    case "mcode":
      return "сессия M Code";
    case "terminal":
      return `терминал ${o.app}`;
    case "launch_agent":
      return `LaunchAgent ${o.label}`;
    case "orphan":
      return "сирота (родитель завершился)";
    case "unknown":
      return o.top;
  }
}

/** "проект M Code, Claude Code «Старт»" */
export function authorText(a: Author): string {
  const parts: string[] = [];
  const proj = a.project ?? cwdShort(a.git_root ?? a.cwd);
  if (proj) parts.push(`проект ${proj}`);
  if (a.lane) parts.push(`полоса ${a.lane}`);
  parts.push(originText(a.origin));
  return parts.join(", ");
}

function modeText(m: GuardMode): string {
  return m.kind === "standby" || m.kind === "broken" ? `${MODE_LABEL[m.kind]}: ${m.detail}` : MODE_LABEL[m.kind];
}

interface Props {
  onToast: (ok: boolean, text: string) => void;
}

export function GuardTab({ onToast }: Props) {
  const { status, error, saving } = useGuard();

  async function patchGuard(patch: Partial<GuardSettings>, done?: string): Promise<boolean> {
    try {
      await guardStore.patch(patch);
      if (done) onToast(true, done);
      return true;
    } catch (e) {
      onToast(false, errText(e));
      return false;
    }
  }

  async function openConfig() {
    try {
      await (await backend()).openConfig();
    } catch (e) {
      onToast(false, errText(e));
    }
  }

  if (!status) {
    return <div className="empty">{error ? <span className="error">Сторож недоступен: {error}</span> : "Загрузка…"}</div>;
  }

  const { mode, config: cfg } = status;

  return (
    <div className="pane-scroll">
      <div className="guard">
        <section className="guard-head">
          <div className="guard-mode">
            <div className="dim small">Сторож памяти</div>
            <div className={`mode-word mode-${mode.kind}`}>{MODE_LABEL[mode.kind]}</div>
            <div className="dim">{mode.kind === "standby" || mode.kind === "broken" ? mode.detail : MODE_HINT[mode.kind]}</div>
            <div className="zone-line">
              {status.zone ? (
                <>
                  <span className={`zone-dot zone-${status.zone}`} />
                  Зона <b>{ZONE_LABEL[status.zone]}</b>
                  {status.zone_why && <span className="dim"> — {status.zone_why}</span>}
                </>
              ) : (
                <span className="dim">Зона не отслеживается</span>
              )}
            </div>
            {status.zone === "red" && (
              <div className="red-bar" title="Сторож снимает, только если красное держится достаточно долго">
                <span>
                  Красное держится <b>{status.red_for_secs} с</b> из {cfg.sustain_secs}
                </span>
                <span className="meter">
                  <span style={{ width: `${Math.min(100, (status.red_for_secs / Math.max(1, cfg.sustain_secs)) * 100)}%` }} />
                </span>
              </div>
            )}
          </div>
          <div className="guard-switches">
            <Switch
              big
              label="Сторож включён"
              checked={cfg.enabled}
              disabled={saving}
              onChange={(v) => patchGuard({ enabled: v }, v ? "Сторож включён" : "Сторож выключен")}
            />
            <Switch
              label="Режим пробы"
              hint="только пишет, кого снял бы"
              checked={cfg.dry_run}
              disabled={saving || !cfg.enabled}
              onChange={(v) => patchGuard({ dry_run: v }, v ? "Режим пробы: никого не снимает" : "Боевой режим")}
            />
          </div>
        </section>

        <OrphanCleanupCard
          cfg={cfg.orphans}
          watch={status.orphans}
          guardActing={mode.kind === "armed" || mode.kind === "dry_run"}
          dryRun={mode.kind === "dry_run"}
          saving={saving}
          apply={patchGuard}
        />

        <GuardSettingsPanel cfg={cfg} configFile={status.config_file} saving={saving} onOpenConfig={openConfig} onToast={onToast} />

        <section>
          <h3 className="sec-title">События</h3>
          {status.recent.length ? (
            <ul className="gfeed">
              {status.recent.map((e, i) => (
                <EventRow key={`${e.ts}-${e.ev}-${i}`} e={e} />
              ))}
            </ul>
          ) : (
            <div className="dim">Событий пока нет.</div>
          )}
        </section>
      </div>

    </div>
  );
}

const EventRow = memo(function EventRow({ e }: { e: GuardEvent }) {
  const [open, setOpen] = useState(false);
  const time = <span className="gtime dim">{fmtWhen(e.ts)}</span>;
  switch (e.ev) {
    case "kill": {
      const failed = e.error != null || !["term", "kill"].includes(e.outcome);
      return (
        <li className={failed ? "gev ev-kill failed" : "gev ev-kill"}>
          {time}
          <div className="gbody">
            <div>
              <b>
                Снят {e.victim.kind} {fmtBytes(e.victim.mem)}
              </b>{" "}
              <span className="dim">pid {e.victim.pid}</span> — {OUTCOME_LABEL[e.outcome] ?? e.outcome}
              {e.error && <span className="error"> · {e.error}</span>}
            </div>
            <div title={e.author.chain.map((c) => `${c.pid} ${c.name}: ${c.args}`).join("\n")}>{authorText(e.author)}</div>
            <div className="dim small">
              {e.why} · <code>{e.victim.cmd}</code>
            </div>
          </div>
        </li>
      );
    }
    case "would-kill":
      return (
        <li className="gev ev-would">
          {time}
          <div className="gbody">
            <div>
              <b>
                Проба: снял бы {e.victim.kind} {fmtBytes(e.victim.mem)}
              </b>{" "}
              <span className="dim">pid {e.victim.pid}</span>
            </div>
            <div title={e.author.chain.map((c) => `${c.pid} ${c.name}: ${c.args}`).join("\n")}>{authorText(e.author)}</div>
            <div className="dim small">
              {e.why} · <code>{e.victim.cmd}</code>
            </div>
          </div>
        </li>
      );
    case "orphan-kill": {
      const failed = !e.dry_run && (e.error != null || !["term", "kill"].includes(e.outcome));
      return (
        <li className={e.dry_run ? "gev ev-would" : failed ? "gev ev-kill failed" : "gev ev-kill"}>
          {time}
          <div className="gbody">
            <div>
              <b>
                {e.dry_run ? "Проба: сняла бы сироту" : "Снята сирота"} {e.victim.kind} {fmtBytes(e.victim.mem)}
              </b>{" "}
              <span className="dim">pid {e.victim.pid}</span>
              {!e.dry_run && <> — {OUTCOME_LABEL[e.outcome] ?? e.outcome}</>}
              {e.error && <span className="error"> · {e.error}</span>}
            </div>
            <div title={e.author.chain.map((c) => `${c.pid} ${c.name}: ${c.args}`).join("\n")}>{authorText(e.author)}</div>
            <div className="dim small">
              простой {fmtSecs(e.idle_secs)} · <code>{e.victim.cmd}</code>
            </div>
          </div>
        </li>
      );
    }
    case "no-victim":
      return (
        <li className="gev ev-novictim">
          {time}
          <div className="gbody">
            <div>
              Красная зона, но снимать некого <span className="dim">— {e.why}</span>
            </div>
            {e.rejected.length > 0 && (
              <button className="btn-link" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
                {open ? "Скрыть" : `Крупные, кого не тронул, и почему (${e.rejected.length})`}
              </button>
            )}
            {open && (
              <ul className="rejected">
                {e.rejected.map((r) => (
                  <li key={r.pid}>
                    <span className="dim">{r.pid}</span> <b>{r.name}</b> {fmtBytes(r.mem)} <span className="dim">— {r.why}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </li>
      );
    case "zone":
      return (
        <li className="gev quiet">
          {time}
          <div className="gbody">
            <span className={`zone-dot zone-${e.zone}`} /> Зона {ZONE_LABEL[e.zone]}
            {e.why && <span className="dim"> — {e.why}</span>}
          </div>
        </li>
      );
    case "start":
    case "mode":
      return (
        <li className="gev quiet">
          {time}
          <div className="gbody">
            {e.ev === "start" ? "Сторож запущен" : "Режим"}: {modeText(e.mode)}
          </div>
        </li>
      );
    case "limit":
      return (
        <li className="gev quiet">
          {time}
          <div className="gbody">Предел снятий: {e.why}</div>
        </li>
      );
    case "config-error":
      return (
        <li className="gev quiet">
          {time}
          <div className="gbody">
            <span className="error">Ошибка настроек:</span> {e.error}
          </div>
        </li>
      );
  }
});
