Ты — исполнитель полосы CORE, задача 01. Каталог: /Users/leoerdman/Programming - Main/Resource Management (правь здесь).
Отвечай по-русски.

Цель: в репозитории есть Cargo workspace с библиотекой `rmon-core` (сбор памяти и процессов на macOS, история, анализ подозреваемых, защитный слой снятия) и бинарём `rmon` (`status`, `ps`, `suggest`, `kill`, у всех `--json`), и его цифры совпадают с Activity Monitor / `vm_stat` / `memory_pressure`.
Зачем: это ядро утилиты-монитора оперативной памяти. Поверх него следующими полосами пойдут MCP-сервер для Claude Code и окно на egui, поэтому публичный API ядра — контракт. Владелец работает с нейросетями на 24 ГБ, ноутбук живёт на ОЗУ; сейчас своп ~9 ГБ и сжатие ~9 ГБ.

СНАЧАЛА прочитай docs/SPEC.md целиком: модель данных, анализ, защитный слой и требования там. Типы и имена полей бери оттуда дословно. Если что-то в спецификации невыполнимо или неверно — сделай как правильно и опиши расхождение в debt, а не молча.

Твои файлы: Cargo.toml (workspace), Cargo.lock, rust-toolchain.toml (не обязателен), crates/rmon-core/**, crates/rmon-cli/**.
Не трогать: docs/** (кроме того, что прямо сказано ниже), README.md, .gitignore, apps/**, crates/rmon-mcp/** — это следующие полосы. Не трогать ничего вне каталога проекта; ~/Programming - Main/Hooks/memory-guard — только читать (его config.json — образец списков never/allow).

Контракт шва (для следующих полос):
- `rmon_core::collect::Collector` — `Collector::new() -> Result<Self>`, `collector.snapshot() -> Result<Snapshot>` (второй и следующие вызовы дают cpu_pct по разнице).
- `rmon_core::history::History` — `push(Snapshot)`, окно по времени; `rmon_core::analyze::analyze(&History, &Thresholds) -> Vec<Suspect>`; `rmon_core::analyze::app_groups(&Snapshot) -> Vec<AppGroup>`.
- `rmon_core::kill::{policy_for(&ProcessInfo) -> KillPolicy, kill(&ProcessInfo, KillRequest) -> Result<KillOutcome>}`; `KillRequest { signal_term_grace: Duration, confirm: bool, requested_by: Requester /* Cli|Mcp|App */, reason: String, dry_run: bool }`.
- Все публичные типы — `serde::Serialize` (+ `Deserialize`, где несложно).
- Трейт `Platform` в `rmon_core::platform`, реализация `platform::macos` под `#[cfg(target_os = "macos")]`; на других ОС крейт должен компилироваться с понятной ошибкой «платформа пока не поддерживается» во время выполнения, а не при сборке.

Шаги:
1. Workspace: `Cargo.toml` с members `crates/rmon-core`, `crates/rmon-cli`; edition 2024; `[profile.release]` с `lto = "thin"`, `strip = true`, `opt-level = 3`. Зависимости минимальные: `libc`, `mach2` (если нужен), `serde`, `serde_json`, `thiserror`/`anyhow`, `clap` (derive) — только в cli. Никаких `tokio` в ядре, никакого `sysinfo`.
2. `platform/macos.rs`: `proc_listallpids`, `proc_pidinfo(PROC_PIDTBSDINFO)` (ppid, uid, status, start time), `proc_pid_rusage(RUSAGE_INFO_V4)` → `ri_phys_footprint`, `ri_resident_size`, CPU-время; `proc_pidinfo(PROC_PIDTASKINFO)` → threads; `proc_pidpath`; `sysctl KERN_PROCARGS2` → args без окружения; `host_statistics64(HOST_VM_INFO64)` + `sysctl` `hw.memsize`, `hw.pagesize`, `vm.swapusage`, `kern.memorystatus_vm_pressure_level`, `kern.memorystatus_level`. Ошибка прав на чужом процессе — поле `None`/пусто, не падение. На каждый `unsafe` — комментарий, почему безопасно.
3. `app_group`: процесс внутри `/Applications/X.app/` или `…/X.app/Contents/` → «X» (хелперы Chrome → «Google Chrome», хелперы M Code → «M Code Desktop»); иначе имя процесса.
4. `history.rs`, `analyze.rs`: подозреваемые Hog / Growing / Orphan / Idle / Duplicate / Zombie по SPEC, `reason` по-русски с цифрами, `score` 0–100. Пороги — в `Thresholds` с `Default`. Юнит-тесты на анализ — на синтетических снимках (без реальной системы).
5. `kill.rs`: политика Protected / Confirm / Free (списки не мягче `never` из memory-guard/config.json; плюс kernel_task, launchd, WindowServer, loginwindow, pid 0 и 1, сам текущий процесс). Перед каждым сигналом — сверка `(pid, started_at, args)`. SIGTERM → ожидание → SIGKILL. Журнал `~/Library/Logs/rmon/actions.jsonl` (создать каталог). Юнит-тест: породить `sleep 300`, снять его через `kill`, убедиться, что умер; тест отказа на pid 1 и на подменённом `started_at`.
6. `rmon-cli` (бинарь `rmon`):
   - `rmon status` (и `rmon` без аргументов): давление словом и цветом, использовано/всего, app/wired/compressed/cached/free, своп, топ-10 приложений по сумме footprint, топ-5 подозреваемых (по двум снимкам с интервалом 1 с).
   - `rmon ps [--sort mem|cpu|rss|pid] [--limit N] [--group] [--filter TEXT]` — таблица.
   - `rmon suggest [--window 10s] [--interval 2s]` — снимает историю на окне и печатает подозреваемых с политикой и сколько освободится.
   - `rmon kill <PID> [--confirm] [--hard] [--dry-run] [--reason TEXT]` — через защитный слой; Protected — отказ с причиной и ненулевой код; Confirm без `--confirm` — отказ с подсказкой.
   - `--json` у всех — сериализация структур ядра.
   Размеры — человеку «3,1 ГБ» / «412 МБ».
7. Прогоны держи лёгкими: машина сейчас перегружена (load > 100, 11 сессий). Собирай с `CARGO_BUILD_JOBS=4`, только `cargo build -p …` / `cargo test -p rmon-core`, release-сборку — один раз в конце.

Проба:
```
CARGO_BUILD_JOBS=4 cargo test -p rmon-core
CARGO_BUILD_JOBS=4 cargo build --release -p rmon-cli
./target/release/rmon status
./target/release/rmon --json status | head -c 600
vm_stat; memory_pressure | tail -1; sysctl vm.swapusage
./target/release/rmon ps --limit 15
top -l 1 -o mem -n 15 -stats pid,command,mem
./target/release/rmon suggest --window 6s --interval 2s
/usr/bin/time -l ./target/release/rmon status 2>&1 | grep -E "real|maximum resident"
sleep 300 & ./target/release/rmon kill $! ; ./target/release/rmon kill 1 ; echo "exit=$?"
```
Сверь: wired/compressed/своп/давление расходятся с `vm_stat`/`sysctl` не больше чем на 2 % (снимки в разное время); footprint топ-процессов совпадает с колонкой MEM у `top` с точностью до округления.
Если пришёл отказ «Сторож машины … НЕ запущена» — подожди 60 с и повтори (до 5 раз): это очередь тяжёлых прогонов, не ошибка.
Негативный контроль: временно поменяй в `policy_for` проверку kernel_task/launchd так, чтобы pid 1 стал Free, запусти `rmon kill 1 --dry-run` — должен показать «снял бы»; затем верни как было и убедись, что снова отказ. Опиши в verify, что видел.

Правила:
- не коммить, не пушь (это делает оркестратор после приёма);
- не создавай субагентов и сессий;
- вне цели ничего не чини — впиши в debt;
- никаких реальных kill чужих процессов — только своих подставных `sleep`;
- в конце `cargo clippy -p rmon-core -p rmon-cli -- -D warnings` должен быть чистым.

Последним в ответе — ровно этот блок:
RESULT
status: done | blocked | partial
changed: <файлы через запятую | none>
verify: <команды -> итог; цифры rmon против vm_stat/top; что показал негативный контроль; время и RSS `rmon status`>
api: <публичные функции/типы rmon-core, на которые могут опереться полосы MCP и APP, одной строкой каждый>
debt: <по строке на находку с приоритетом P0–P3 | none>
END
