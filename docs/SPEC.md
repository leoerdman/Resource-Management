# rmon — спецификация

## Зачем

Владелец работает с нейросетями и агентами (Claude Code, M Code, десятки `node`/`claude`/`bun`
процессов) на MacBook M4 Pro с 24 ГБ. Ноутбук держится на оперативной памяти: своп 9+ ГБ,
сжатие 8+ ГБ — обычное дело. Activity Monitor показывает цифры, но не отвечает на главные вопросы:
**кто съел память, растёт ли он, нужен ли он ещё и можно ли его снять безопасно.**

`rmon` отвечает на эти вопросы: в окне — человеку, через MCP — Claude Code, который может снимать
процессы через защитный слой.

## Отношение к `memory-guard`

На машине уже работает `~/Programming - Main/Hooks/memory-guard` (LaunchAgent
`com.leoerdman.memory-guard`): в красной зоне он сам снимает самый большой `tsgo`/`node`/`bun`-прогон.
`rmon` его **не заменяет и с ним не спорит**: сам ничего не убивает автоматически, только показывает,
предлагает и выполняет снятие по явной команде (человека или Claude Code). Журнал сторожа
(`log/events.jsonl`) `rmon` читает и показывает. Списки «никогда не трогать» у `rmon` не мягче,
чем `never` в `memory-guard/config.json`.

## Архитектура

```
Cargo workspace (edition 2024)
crates/rmon-core   библиотека: сбор, история, анализ, защитный слой снятия
crates/rmon-cli    бинарь `rmon`: status / ps / suggest / kill / mcp, у всех --json
crates/rmon-mcp    MCP-сервер (stdio) поверх rmon-core; запускается как `rmon mcp`
apps/rmon-app      окно (eframe/egui) + иконка в строке меню; сборка в .app скриптом
```

Платформа — за трейтом `Platform` в `rmon-core` (`platform/macos.rs` — сейчас; `linux.rs`,
`windows.rs` — потом). Всё выше трейта платформонезависимо. На macOS — прямые вызовы
`libproc` / `mach` / `sysctl` через `libc` (+ `mach2` при необходимости), без `sysinfo`:
он не даёт `phys_footprint`, а именно его показывает Activity Monitor в столбце «Память».

## Модель данных (контракт для всех полос)

```rust
pub enum Pressure { Normal, Warning, Critical }          // kern.memorystatus_vm_pressure_level 1/2/4

pub struct SystemMemory {
    pub total: u64,            // hw.memsize
    pub used: u64,             // app + wired + compressed (как «Memory Used» в Activity Monitor)
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
- **Duplicate** — много одинаковых строк запуска (например, 6 одинаковых MCP-серверов от старых сессий).
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

Снятие: сверить `(pid, started_at, args)` со снимком (pid мог смениться) → SIGTERM → ожидание
(по умолчанию 5 с) → SIGKILL, если ещё жив. Каждое действие и отказ — строкой JSON в
`~/Library/Logs/rmon/actions.jsonl` (кто попросил: `cli` / `mcp` / `app`, причина, сколько освободили).
Списки — в `rmon-core`, одним местом; можно расширить конфигом `~/.config/rmon/config.toml`.

## Требования

- Снимок ~800 процессов < 100 мс; `rmon status` целиком < 300 мс.
- CLI: RSS < 30 МБ. Окно: RSS < 100 МБ, CPU в покое < 2 % при обновлении раз в 2 с.
- Без прав root; что недоступно — `None`, а не падение.
- Проект не должен сам стать «хавающим память» процессом.

## Фазы (полосы M Code)

1. **CORE** — workspace, `rmon-core` (macOS), `rmon-cli`: `status`, `ps`, `suggest`, `kill`. → [tasks/01-core-cli.md](tasks/01-core-cli.md)
2. **MCP** — `rmon-mcp`, `rmon mcp`, чтение журнала `memory-guard`, инструкция подключения к Claude Code.
3. **APP** — окно на eframe, иконка давления в строке меню, `scripts/bundle-macos.sh` → `dist/Resource Monitor.app`.

Фазы 2 и 3 зависят только от публичного API `rmon-core` и могут идти параллельно после фазы 1.
