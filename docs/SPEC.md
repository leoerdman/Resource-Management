# rmon — спецификация

## Зачем

Владелец работает с нейросетями и агентами (Claude Code, M Code, десятки `node`/`claude`/`bun`
процессов) на MacBook M4 Pro с 24 ГБ. Ноутбук держится на оперативной памяти: своп 9+ ГБ,
сжатие 8+ ГБ — обычное дело. Activity Monitor показывает цифры, но не отвечает на главные вопросы:
**кто съел память, растёт ли он, нужен ли он ещё и можно ли его снять безопасно.**

`rmon` отвечает на эти вопросы: в окне — человеку, через MCP — Claude Code, который может снимать
процессы через защитный слой.

## Сторож памяти (бывший `memory-guard`)

Раньше на машине работал отдельный демон `~/Programming - Main/Hooks/memory-guard` (Bun, LaunchAgent
`com.leoerdman.memory-guard`): в красной зоне он сам снимал самый большой `tsgo`/`node`/`bun`-прогон.
С 2026-11-20 он **перенесён в rmon** (`rmon_core::guard`, решение владельца: «всё через наше
приложение») — та же логика, проверенная теми же сценариями, плюс:

- работает внутри Resource Monitor на общем `Monitor` (или без окна: `rmon guard run`);
- **один сторож на машину** — файловая блокировка `guard.lock`; пока жив старый LaunchAgent, сторож
  rmon ждёт (`standby`), окно предлагает «перенести» (агент выгружается, plist → `*.disabled-by-rmon`,
  обратимо: `rmon guard restore`);
- жертва должна пройти и списки сторожа, и политику снятия rmon (`Free`) — двойной забор;
- уведомления — системные, из приложения; все настройки сторожа (режим, красная зона, жертва,
  пределы, уведомления) — крутилки во вкладке «Сторож», каждая сразу пишет `config.toml` и
  применяется до ответа; ещё переключатель в полосе вкладок, меню значка, `rmon guard on|off|dry|arm`;
- кто держит сторожа — решает сама блокировка, а не pid в файле (после снятия там остаётся прежний);
- настройки — секция `[guard]` в `config.toml` (по умолчанию — значения старого `config.json`);
  `[guard] never` — и «никогда» для сторожа, и «с подтверждением» для ручного снятия;
- журнал — `guard.jsonl` в каталоге журналов rmon; история старого сторожа видна во вкладке «Журнал»;
- упор в предел снятий (`limits`) — с уведомлением: прогоны перезапускаются быстрее, чем их снимают.
- **автоочистка сирот** (`[guard.orphans]`, по умолчанию включена, 5 мин): сирота — то же правило,
  что у подозреваемых (`analyze::is_orphan`: dev-процесс, ppid 1, от `orphan_bytes`); если на каждом
  опросе CPU ≤ `idle_cpu_pct` (1 %) и память в пределах max(4 МБ, 2 %) `idle_mins` минут подряд —
  сторож снимает её в любой зоне тем же защитным слоем (Free-политика, «никогда», сверка личности),
  не больше 3 за опрос, одно уведомление на пачку; задачи LaunchAgent пропускает (launchd их
  перезапустит). В пробе — событие `orphan-kill` с `dry_run`. Переключатель и минуты — вторая
  карточка вкладки «Сторож».

## Журналы

Все журналы — JSONL в каталоге журналов (`rmon config` покажет), хранятся по времени: `[logs]
retention_hours = 24` (страховка по размеру — `max_mb`). `actions.jsonl` — каждое снятие и отказ,
`guard.jsonl` — решения сторожа, `timeline.jsonl` — раз в `timeline_every_secs` (60 с) состояние
памяти, топ процессов, подозреваемые и режим сторожа: по ней разбирают «почему не снял» задним числом.

Зона, гистерезис, лимиты, выбор жертвы и «кто автор» — см. doc-комментарий `guard/mod.rs`.

## Архитектура

```
Cargo workspace (edition 2024)
crates/rmon-core      библиотека: сбор (Platform), история, анализ, политика, снятие, Monitor, конфиг
crates/rmon-cli       бинарь `rmon`: status / ps / suggest / why / kill / config / mcp, у всех --json
crates/rmon-mcp       MCP-сервер (stdio) поверх rmon-core; запускается как `rmon mcp`
apps/rmon-app         Tauri 2: окно (React + Vite, src/) и значок в строке меню (src-tauri/)
scripts/              prepare-sidecar.mjs (CLI внутрь бандла), install-macos.sh
.github/workflows/    CI на macOS / Linux / Windows, релиз .dmg / .msi / .AppImage
```

**Почему Tauri, а не egui** (решение 2026-11-20): владельцу нужен мультиплатформенный
устанавливаемый апп. Tauri даёт значок в строке меню, установщики `.dmg`/`.msi`/`.AppImage`/`.deb`
и CLI-sidecar внутри бандла на всех трёх ОС, а интерфейс на React собирается и меняется быстрее,
чем на egui. Цена — процесс WebView (~50 МБ); вся логика при этом остаётся в Rust.

**Один источник истины:**
- `rmon_core::monitor` — единственный цикл сбора: снимок → история анализа (облегчённая,
  `History::slim`) → `analyze` → `Dashboard`. Окно и MCP-сервер берут `MonitorHandle`
  (`dashboard`, `details`, `kill`), CLI — разовый `Monitor::tick`. Своих циклов ни у кого нет.
- `rmon_core::policy` — единственное место решения «можно ли снимать» (см. ниже).
- `rmon_core::config` — настройки и все пути (журналы, конфиг, memory-guard) по ОС.
- `rmon_core::fmt` — «3,1 ГБ». Окно форматирует тем же правилом: `cargo test` пишет эталон
  `apps/rmon-app/src/bindings/format-cases.json`, `npm test` сверяет с ним `fmtBytes`.
- TS-типы окна генерирует ts-rs из Rust (`apps/rmon-app/src/bindings/*.ts`), подписи
  перечислений — `bindings/labels.json`. CI падает, если сгенерированное не закоммичено.

Платформа — за трейтом `Platform` в `rmon-core`: `platform/macos.rs` (libproc / mach / sysctl
через `libc`, без `sysinfo` — он не даёт `phys_footprint`), `platform/linux.rs` (`/proc`, PSI),
`platform/windows.rs` (`windows-sys`). Чужие процессы без прав остаются в снимке с `footprint = None`
(на macOS — через `PROC_PIDT_SHORTBSDINFO` и `sysctl KERN_PROC_PID`).

## Модель данных (контракт для всех полос)

```rust
pub enum Pressure { Normal, Warning, Critical }          // kern.memorystatus_vm_pressure_level 1/2/4

pub struct SystemMemory {
    pub total: u64,            // hw.memsize
    pub used: u64,             // total - free - cached_files (как «Memory Used» в Activity Monitor)
    pub other: u64,            // used - app - wired - compressed: спекулятивные/переиспользуемые страницы
    pub app: u64,              // (internal - purgeable) * page
    pub wired: u64,
    pub compressed: u64,       // compressor_page_count * page
    pub cached_files: u64,     // (external + purgeable) * page
    pub free: u64,
    pub swap_total: u64,
    pub swap_used: u64,        // vm.swapusage
    pub pressure: Pressure,
    pub available_pct: u8,     // kern.memorystatus_level
    pub counters: VmCounters,  // pageins, pageouts, swapins, swapouts, compressions, decompressions — накопительные
}

pub enum ProcState { Running, Sleeping, Stopped, Zombie, Unknown }

pub struct ProcessInfo {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub exe: Option<String>,        // proc_pidpath
    pub args: Vec<String>,          // KERN_PROCARGS2, без окружения
    pub uid: u32,
    pub footprint: Option<u64>,     // ri_phys_footprint; None, если нет прав (процессы root)
    pub rss: u64,
    pub cpu_pct: f32,               // по разнице двух снимков; в первом снимке 0
    pub threads: u32,
    pub state: ProcState,
    pub started_at: u64,            // unix-секунды; вместе с pid — личность процесса
    pub app_group: String,          // «Google Chrome», «M Code Desktop», «Claude», иначе имя процесса
}

pub struct Snapshot { pub taken_at: u64, pub system: SystemMemory, pub processes: Vec<ProcessInfo> }
```

Все типы — `serde::Serialize`; JSON CLI и MCP — эти же структуры.

## Анализ: подозреваемые

`History` — кольцо снимков (по умолчанию 10 мин). По нему `Analyzer` выдаёт `Suspect`:

```rust
pub enum SuspectKind { Hog, Growing, Orphan, Idle, Duplicate, Zombie }
pub struct Suspect {
    pub pid: i32, pub kind: SuspectKind,
    pub score: u8,                 // 0–100, по нему сортировка
    pub name: String, pub app_group: String, pub started_at: u64,
    pub cmd: String,               // строка запуска ≤ 200 символов
    pub group: Option<String>,     // общий ключ у брошенных копий одной команды
    pub reason: String,            // по-русски, человеку: «node 3,1 ГБ, растёт 40 МБ/мин 6 мин подряд»
    pub reclaim: u64,              // сколько освободится (footprint, иначе rss)
    pub policy: KillPolicy,        // см. ниже
}
```

- **Hog** — большой footprint (порог настраиваемый, по умолчанию ≥ 1 ГБ или ≥ 5 % ОЗУ).
- **Growing** — монотонный рост на окне (наклон, а не одна точка): утечка.
- **Orphan** — dev-процесс (`node`, `bun`, `python`, `tsgo`, `claude`, `cargo`, `rustc`…) с `ppid == 1`,
  то есть родитель умер, а он остался.
- **Idle** — большой и почти без CPU долго (зависшая команда, забытый REPL/сервер).
- **Duplicate** — брошенные копии одной dev-команды: ≥ 3 одинаковых строки запуска у сирот
  (например, MCP-серверы, пережившие свои сессии). Копии под живым родителем дублями **не** считаются:
  это пулы воркеров, сессии Claude Code внутри приложения Claude, MCP-сервер на каждую сессию.
  У дублей общий `group` — окно сворачивает их в одну строку с «Снять все».
- **Zombie** — `ProcState::Zombie`.

Суммарно по приложениям — `AppGroup { name, pids, footprint, count }`.

## Защитный слой снятия

```rust
pub enum KillPolicy {
    Protected(String),   // никогда: системные пути, kernel_task, launchd, WindowServer, loginwindow, сам rmon
    Confirm(String),     // приложения владельца и их хелперы: Claude, M Code Desktop, браузеры, Telegram,
                         // редакторы, сессии `claude` CLI, mcode-engine — только с явным подтверждением
    Free,                // dev-прогоны и прочее
}
```

Правила — данные (`RuleSpec { name, exe?, path?, args? }`, регулярные выражения — та же форма,
что у `memory-guard/config.json`), решение принимает `rmon_core::policy::Policy` в таком порядке:
1. pid ядра/init и сам rmon → `Protected`;
2. процесс другого пользователя (мы не root) → `Protected` («нет прав»);
3. встроенные «никогда» по ОС (на macOS: `/System`, `/usr/libexec`, `/usr/sbin`, `/sbin`,
   `/Library/Apple`, `kernel_task`/`launchd`/`WindowServer`/`loginwindow`, сторожа владельца) ∪
   `[[policy.protect]]` из конфига → `Protected`;
4. встроенные «с подтверждением» (приложения владельца, `claude`, `mcode-engine`, утилиты `/bin` и
   `/usr/bin`, любой `.app`) ∪ **весь `never` из `memory-guard/config.json`** ∪ `[[policy.confirm]]`
   → `Confirm`;
5. иначе `Free`.

Так «никогда» сторожа у rmon не мягче: сторож не трогает их автоматически, rmon — без явного
согласия человека. Конфиг может только ужесточить политику — ключа «освободить» в нём нет.

Снятие: сверить `(pid, started_at, args)` со свежим чтением (pid мог смениться) → SIGTERM → ожидание
(по умолчанию 5 с) → повторная сверка → SIGKILL, если ещё жив. Каждое действие и отказ — строкой
JSON в журнал (`rmon config` покажет путь; на macOS `~/Library/Logs/rmon/actions.jsonl`; кто попросил:
`cli` / `mcp` / `app`, причина, сколько освободили).

Настройки — `~/.config/rmon/config.toml` (Windows: `%APPDATA%\rmon\config.toml`; env `RMON_CONFIG`):
секции `[thresholds]`, `[[policy.protect]]`, `[[policy.confirm]]`, `[policy] use_memory_guard`,
`[paths]`. Неизвестный ключ — ошибка (`rmon config` её покажет).

## Требования

- Снимок ~800 процессов < 100 мс; `rmon status` целиком < 300 мс.
- CLI: RSS < 30 МБ. Окно: RSS < 100 МБ, CPU в покое < 2 % при обновлении раз в 2 с.
- Без прав root; что недоступно — `None`, а не падение.
- Проект не должен сам стать «хавающим память» процессом: окно получает сводку через Tauri
  `Channel` и только пока видно; значок трея перерисовывается, лишь когда изменилось видимое.
- CLI: коды выхода 0 — успех, 1 — ошибка, 2 — снятие запрещено, 3 — нужно `--confirm`,
  4 — процесс не найден/завершился/подменился.

## Фазы

1. **CORE** — `rmon-core`, `rmon-cli`. → [tasks/01-core-cli.md](tasks/01-core-cli.md) — сделано.
2. **MCP** — `rmon-mcp`, `rmon mcp`, инструкция [CLAUDE_CODE.md](CLAUDE_CODE.md). → [tasks/02-mcp.md](tasks/02-mcp.md)
3. **APP** — Tauri 2 вместо eframe (см. «Почему Tauri»), установка `scripts/install-macos.sh`.
   → [tasks/03-app.md](tasks/03-app.md) — исходная постановка под egui, цели те же.
4. **GUARD** — перенос memory-guard в rmon, уведомления, автозапуск при входе, вкладка «Настройки».
5. **PLATFORMS** — Linux и Windows: отложено, см. [ROADMAP.md](ROADMAP.md).
