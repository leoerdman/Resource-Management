import { memo, useCallback, useEffect, useState } from "react";
import { backend } from "../api";
import { cwdShort, errText, fmtBytes, fmtClock, fmtDateTime } from "../format";
import type { Author, GuardConfig, GuardEvent, GuardMode, GuardSettings, GuardStatus, GuardZone, Origin } from "../types";
import { ConfirmDialog } from "./ConfirmDialog";
import { Switch } from "./Controls";

const POLL_MS = 3000;
const MB = 1024 * 1024;

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

export const LEVEL_LABEL: Record<number, string> = { 1: "нормальное", 2: "жёлтое", 4: "красное" };

/** "45 с", "10 мин", "1 ч", "1 ч 30 мин". */
export function fmtSecs(s: number): string {
  if (s < 60) return `${s} с`;
  if (s < 3600) return s % 60 ? `${Math.floor(s / 60)} мин ${s % 60} с` : `${s / 60} мин`;
  const m = Math.round(s / 60);
  return m % 60 ? `${Math.floor(m / 60)} ч ${m % 60} мин` : `${m / 60} ч`;
}

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

type Confirm = "retire" | "restore" | null;

export function GuardTab({ onToast }: Props) {
  const [status, setStatus] = useState<GuardStatus | null>(null);
  const [guard, setGuard] = useState<GuardSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirm, setConfirm] = useState<Confirm>(null);

  const refresh = useCallback(async () => {
    try {
      const b = await backend();
      setStatus(await b.guardStatus());
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  }, []);

  // Status now, on every guard event, and every few seconds («красное держится N с» ticks).
  useEffect(() => {
    let alive = true;
    let off: (() => void) | null = null;
    backend()
      .then(async (b) => {
        const s = await b.getSettings();
        if (alive) setGuard(s.guard);
        const unsub = await b.onGuardEvent(() => refresh());
        if (alive) off = unsub;
        else unsub();
      })
      .catch((e) => alive && setError(errText(e)));
    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => {
      alive = false;
      clearInterval(id);
      off?.();
    };
  }, [refresh]);

  async function patchGuard(patch: Partial<GuardSettings>, done: string) {
    setSaving(true);
    try {
      const b = await backend();
      const cur = await b.getSettings();
      const saved = await b.saveSettings({ ...cur, guard: { ...cur.guard, ...patch } });
      setGuard(saved.guard);
      onToast(true, done);
      await refresh();
    } catch (e) {
      onToast(false, errText(e));
    } finally {
      setSaving(false);
    }
  }

  async function legacyAction(which: "retire" | "restore") {
    try {
      const b = await backend();
      setStatus(which === "retire" ? await b.retireLegacy() : await b.restoreLegacy());
      onToast(true, which === "retire" ? "Старый сторож выгружен — теперь сторожит Resource Monitor" : "Старый сторож возвращён");
      setConfirm(null);
    } catch (e) {
      onToast(false, errText(e));
      throw e;
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

  const { mode, config: cfg, legacy } = status;
  const enabled = guard?.enabled ?? cfg.enabled;
  const dryRun = guard?.dry_run ?? cfg.dry_run;

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
              checked={enabled}
              disabled={saving}
              onChange={(v) => patchGuard({ enabled: v }, v ? "Сторож включён" : "Сторож выключен")}
            />
            <Switch
              label="Режим пробы"
              hint="только пишет, кого снял бы"
              checked={dryRun}
              disabled={saving || !enabled}
              onChange={(v) => patchGuard({ dry_run: v }, v ? "Режим пробы: никого не снимает" : "Боевой режим")}
            />
          </div>
        </section>

        {legacy.kind === "running" && (
          <section className="banner banner-warn">
            <div>
              Работает старый сторож <b>memory-guard</b> (LaunchAgent{legacy.pid != null ? `, pid ${legacy.pid}` : ""}). Пока он
              жив, сторож Resource Monitor ждёт, чтобы не снимать дважды.
            </div>
            <button className="btn btn-primary" onClick={() => setConfirm("retire")}>
              Перенести в Resource Monitor
            </button>
          </section>
        )}
        {legacy.kind === "installed" && (
          <section className="banner">
            <div>Старый сторож memory-guard установлен, но сейчас не запущен.</div>
            <button className="btn" onClick={() => setConfirm("retire")}>
              Отключить насовсем
            </button>
          </section>
        )}
        {legacy.kind === "retired" && (
          <div className="dim small legacy-quiet">
            Старый сторож memory-guard перенесён в Resource Monitor.{" "}
            <button className="btn-link" onClick={() => setConfirm("restore")}>
              Вернуть
            </button>
          </div>
        )}

        <HowItWorks cfg={cfg} configFile={status.config_file} onOpenConfig={openConfig} />

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

      {confirm === "retire" && (
        <ConfirmDialog
          title="Перенести сторожа в Resource Monitor?"
          confirmLabel="Перенести"
          busyLabel="Переношу…"
          onCancel={() => setConfirm(null)}
          onConfirm={() => legacyAction("retire")}
        >
          Старый LaunchAgent будет выгружен, его plist переименован в <code>*.disabled-by-rmon</code> — это обратимо. Журнал
          и файлы старого сторожа не трогаются.
        </ConfirmDialog>
      )}
      {confirm === "restore" && (
        <ConfirmDialog
          title="Вернуть старый memory-guard?"
          confirmLabel="Вернуть"
          busyLabel="Возвращаю…"
          onCancel={() => setConfirm(null)}
          onConfirm={() => legacyAction("restore")}
        >
          plist вернётся на место и LaunchAgent снова запустится. Пока он работает, сторож Resource Monitor будет ждать.
        </ConfirmDialog>
      )}
    </div>
  );
}

function HowItWorks({ cfg, configFile, onOpenConfig }: { cfg: GuardConfig; configFile: string; onOpenConfig: () => void }) {
  const red = cfg.red;
  const limits = cfg.limits.map((l) => `${l.max} за ${fmtSecs(Number(l.window_secs))}`).join(", ");
  const kinds = [...new Set(cfg.allow.map((a) => a.name))].join(", ");
  return (
    <section>
      <h3 className="sec-title">Как работает</h3>
      <ul className="how">
        <li>
          <b>Красная зона</b> — давление {LEVEL_LABEL[red.pressure_level] ?? red.pressure_level} или выше, или доступно не
          больше {red.min_available_pct} %, или давление от {LEVEL_LABEL[red.swap_red_from_level] ?? red.swap_red_from_level} и
          своп от {fmtBytes(Number(red.swap_red_mb) * MB)}.
        </li>
        <li>
          Снимает, только если красное держится <b>{fmtSecs(cfg.sustain_secs)}</b> — короткие всплески не в счёт.
        </li>
        <li>
          Жертва — самый большой прогон ({kinds || "по правилам «можно»"}) от <b>{cfg.min_victim_mb} МБ</b>: сначала SIGTERM,
          через {fmtSecs(cfg.term_grace_secs)} — SIGKILL.
        </li>
        <li>Не трогает приложения, сессии и всё из списка «никогда» ({cfg.never.length}).</li>
        <li>
          Не больше {limits || "—"}; после снятия пауза {fmtSecs(cfg.after_kill_secs)}.{" "}
          {cfg.notify ? "О каждом снятии — системное уведомление" : "Уведомления о снятии выключены"}
          {cfg.notify_dry_run ? ", в пробе тоже." : "."}
        </li>
      </ul>
      <details className="rules">
        <summary>
          Правила: «можно» ({cfg.allow.length}) и «никогда» ({cfg.never.length})
        </summary>
        <div className="rules-cols">
          <div>
            <h4>Можно снимать</h4>
            <ul>
              {cfg.allow.map((r, i) => (
                <li key={i}>
                  <b>{r.name}</b> <code>exe {r.exe}</code>
                  {r.args && <code>args {r.args}</code>}
                  {r.args_not && <code>кроме {r.args_not}</code>}
                  {r.parent_not && <code>родитель не {r.parent_not}</code>}
                </li>
              ))}
            </ul>
          </div>
          <div>
            <h4>Никогда</h4>
            <ul>
              {cfg.never.map((r, i) => (
                <li key={i}>
                  <b>{r.name}</b>
                  {r.exe && <code>exe {r.exe}</code>}
                  {r.path && <code>путь {r.path}</code>}
                  {r.args && <code>args {r.args}</code>}
                </li>
              ))}
            </ul>
          </div>
        </div>
      </details>
      <div className="config-line">
        <button className="btn btn-small" onClick={onOpenConfig}>
          Изменить в config.toml
        </button>
        <span className="dim small mono ellipsis" title={configFile}>
          {configFile}
        </span>
      </div>
    </section>
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
