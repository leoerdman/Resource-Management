Ты — исполнитель полосы MCP, задача 02. Каталог: /Users/leoerdman/Programming - Main/Resource Management (правь здесь).
Отвечай по-русски.

Цель: `rmon mcp` поднимает MCP-сервер (stdio), через который Claude Code видит память и процессы, получает предложения, что снять, и снимает процессы через защитный слой `rmon-core`.
Зачем: владелец работает с агентами на 24 ГБ ОЗУ; он хочет сказать Claude Code «разберись с памятью» — и тот сам смотрит, кто растёт, кто завис, кто сирота, предлагает и (с согласия для приложений владельца) снимает.

СНАЧАЛА прочитай docs/SPEC.md и публичный API `crates/rmon-core` (lib.rs и модули). Ядро уже принято — опирайся на него, не переписывай.

Твои файлы: crates/rmon-mcp/** (новый крейт), Cargo.toml (только добавить member и зависимости), Cargo.lock, crates/rmon-cli/src/** (только подключить подкоманду `mcp`), docs/CLAUDE_CODE.md (новый).
Можно в rmon-core: ДОБАВИТЬ (не менять существующее) функцию получения cwd процесса (`proc_pidinfo(PROC_PIDVNODEPATHINFO)`) и функцию дерева детей, если их нет. Больше в ядре ничего.
Не трогать: apps/** (параллельно может работать полоса APP), остальной rmon-core, README.md, docs/SPEC.md.

Шаги:
1. Крейт `rmon-mcp` на официальном SDK `rmcp` (features для server + stdio; tokio `rt` current_thread — только в этом крейте). Если `rmcp` не собирается с нашим toolchain — минимальный JSON-RPC 2.0 по stdio руками (initialize, tools/list, tools/call, ping, notifications/initialized) и строка в debt.
2. Фоновый сэмплер: пока сервер жив, поток снимает `Snapshot` раз в 5 с в `History` (окно 15 мин), чтобы рост считался по минутам, а не по секундам. Сам сервер — лёгкий: RSS < 40 МБ.
3. Инструменты (описания — по-английски для модели, текстовый ответ — по-русски для человека + JSON-структура в `structuredContent` или вторым блоком):
   - `memory_overview` — давление, использовано/всего, app/wired/compressed/cached/free, своп, тренд давления и свопа за окно, топ-10 приложений, топ-5 подозреваемых, текущая зона memory-guard (по последнему событию его журнала).
   - `list_processes {sort: mem|cpu|growth|pid = mem, limit = 25, filter?: string, group_by_app = false}`.
   - `process_details {pid}` — ProcessInfo, cwd, родительская цепочка до launchd, дети с суммой памяти, ряд footprint из истории, политика снятия и её причина.
   - `suggest_cleanup {min_score = 30}` — подозреваемые с reason, reclaim, policy; в начале — итог «можно освободить ≈ X ГБ без подтверждения, ещё Y ГБ — с подтверждением».
   - `kill_process {pid, reason, hard = false, confirm = false, dry_run = false}` — только через `rmon_core::kill`, `requested_by = Mcp`. Protected — всегда отказ. Confirm без `confirm=true` — отказ с текстом, что нужно спросить владельца. В описании инструмента прямо: «for Confirm-policy processes (user apps: browsers, Claude, M Code, Telegram, editors, claude CLI sessions) ask the user before calling with confirm=true».
   - `guard_events {limit = 20}` — последние события `~/Programming - Main/Hooks/memory-guard/log/events.jsonl` (путь — константа с переопределением через env `RMON_GUARD_LOG`); если файла нет — пустой список, не ошибка.
4. Подкоманда `rmon mcp` в CLI запускает сервер. Логи — только в stderr (stdout — протокол).
5. docs/CLAUDE_CODE.md: как собрать, как подключить (`claude mcp add --scope user rmon -- "<абсолютный путь>/target/release/rmon" mcp`), список инструментов, пример диалога. САМ в настройки Claude Code НИЧЕГО не прописывай — это решит владелец.

Проба (одним прогоном):
```
CARGO_BUILD_JOBS=4 cargo build --release -p rmon-cli
printf '%s\n' \
 '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}' \
 '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
 '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
 '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory_overview","arguments":{}}}' \
 '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"kill_process","arguments":{"pid":1,"reason":"probe"}}}' \
 | (cat; sleep 3) | ./target/release/rmon mcp
```
Плюс: `sleep 300 &` и `tools/call kill_process {pid: $!, reason: "probe"}` — процесс умер; `kill_process` для pid процесса Claude (`pgrep -x claude | head -1`) без confirm — отказ с просьбой спросить владельца (НЕ передавай confirm=true на реальных процессах!).
Если пришёл отказ «Сторож машины … НЕ запущена» — подожди 60 с и повтори (до 5 раз).
Негативный контроль: подай `tools/call` с несуществующим инструментом и с `pid: "abc"` — должен прийти корректный JSON-RPC error / isError, сервер не падает и отвечает на следующий запрос.

Правила:
- не коммить, не пушь; не создавай субагентов и сессий;
- не трогай настройки Claude Code и чужие процессы (кроме своих `sleep`);
- вне цели ничего не чини — впиши в debt;
- `cargo clippy -p rmon-mcp -p rmon-cli -- -D warnings` чистый; только сборки своих крейтов, `CARGO_BUILD_JOBS=4`.

Последним в ответе — ровно этот блок:
RESULT
status: done | blocked | partial
changed: <файлы через запятую | none>
verify: <команды -> итог; выдержка ответа memory_overview; отказ на pid 1; негативный контроль; RSS сервера>
debt: <по строке на находку с приоритетом P0–P3 | none>
END
