Ты — исполнитель полосы APP, задача 03. Каталог: /Users/leoerdman/Programming - Main/Resource Management (правь здесь).
Отвечай по-русски.

Цель: `dist/Resource Monitor.app` — лёгкое окно на eframe/egui поверх `rmon-core`, которое показывает память понятнее Activity Monitor (давление и его история, разбивка, своп, приложения, процессы, подозреваемые) и позволяет снять процесс через защитный слой; плюс значок давления в строке меню.
Зачем: владелец работает с нейросетями на MacBook 24 ГБ, ноутбук держится на ОЗУ. Ему нужно одним взглядом видеть, кто съел память, кто растёт и что можно снять. Приложение не должно само есть память. Обёртка под macOS минимальная, всё остальное — кросс-платформенный Rust, чтобы потом собрать под Linux/Windows.

СНАЧАЛА прочитай docs/SPEC.md и публичный API `crates/rmon-core`. Ядро уже принято — опирайся на него, не переписывай.

Твои файлы: apps/rmon-app/** (новый крейт), Cargo.toml (только добавить member), Cargo.lock, scripts/bundle-macos.sh (новый), assets/** (иконка, если сделаешь).
Не трогать: crates/** (параллельно работает полоса MCP), README.md, docs/**.

Шаги:
1. Крейт `apps/rmon-app` (бинарь `rmon-app`) на `eframe` (рендер glow или wgpu — что легче по памяти; замерь). Сбор — в отдельном потоке: `Collector` раз в 2 с → `History` (окно 15 мин) → `analyze` → общий `Arc<Mutex<…>>`/канал. UI перерисовывается по приходу данных (`request_repaint`), а не каждый кадр.
2. Окно:
   - Шапка: давление словом и цветом (зелёный/жёлтый/красный), «использовано X из 24 ГБ», своп, сжато; полоса-разбивка App / Wired / Compressed / Cached / Free с подписями; график давления/used/swap за 15 мин (egui_plot или своя линия).
   - Вкладка «Приложения»: группы по `app_group`, сумма footprint, число процессов, раскрытие в процессы.
   - Вкладка «Процессы»: таблица (pid, имя, память footprint, rss, рост МБ/мин, CPU, потоки, возраст, состояние, политика), сортировка по клику на заголовок, фильтр строкой. Длинная таблица — виртуализация строк (`egui_extras::TableBuilder` с `rows`), чтобы 800 процессов не грузили UI.
   - Вкладка «Подозреваемые»: карточки с reason, сколько освободится, политикой; кнопка «Снять»: Free — подтверждение одним кликом в диалоге; Confirm — диалог с предупреждением и именем приложения; Protected — кнопки нет, причина видна. Снятие — `rmon_core::kill` с `requested_by = App`; итог — тостом/строкой.
   - Вкладка «Журнал»: `~/Library/Logs/rmon/actions.jsonl` и события memory-guard (`~/Programming - Main/Hooks/memory-guard/log/events.jsonl`, путь — константа с переопределением env `RMON_GUARD_LOG`; нет файла — пусто).
   Размеры по-человечески («3,1 ГБ»), интерфейс по-русски, тёмная тема по системе.
3. Значок в строке меню (`tray-icon` + `muda`, только под `#[cfg(target_os = "macos")]` или фичей): цветной кружок давления + текст «14,2 ГБ»; меню: «Открыть», «Топ-5 подозреваемых» (только показ), «Выход». Если связка tray-icon с eframe-событийным циклом не взлетает за разумное время — сделай окно без значка и запиши в debt (P2), не трать на это всю сессию.
4. `scripts/bundle-macos.sh`: `cargo build --release -p rmon-app`, собрать `dist/Resource Monitor.app` (Info.plist: CFBundleName, CFBundleIdentifier `com.leoerdman.rmon`, CFBundleExecutable, LSMinimumSystemVersion 13.0, NSHighResolutionCapable), положить рядом бинарь `rmon` из rmon-cli, если он собран. Подпись — ad-hoc `codesign -s -`. `dist/` в .gitignore уже есть.

Проба:
```
CARGO_BUILD_JOBS=4 cargo build --release -p rmon-app
bash scripts/bundle-macos.sh && ls -la "dist/Resource Monitor.app/Contents/MacOS"
open "dist/Resource Monitor.app"; sleep 20
ps -o pid,rss,%cpu,command -p $(pgrep -f "Resource Monitor.app/Contents/MacOS") ; sleep 10; ps -o pid,rss,%cpu -p $(pgrep -f "Resource Monitor.app/Contents/MacOS")
screencapture -x -l $(osascript -e 'tell application "System Events" to get id of first window of (first process whose name contains "rmon")' 2>/dev/null) /tmp/rmon-app.png 2>/dev/null || screencapture -x /tmp/rmon-app.png
```
Посмотри скриншот (прочитай файл картинкой) и опиши, что видно. Требование: RSS < 100 МБ, CPU в покое < 2 %. Закрой приложение в конце (`pkill -f "Resource Monitor.app/Contents/MacOS"`).
Если пришёл отказ «Сторож машины … НЕ запущена» — подожди 60 с и повтори (до 5 раз).
Негативный контроль: в окне попробуй снять подставной `sleep 300` (запусти его заранее, найди фильтром) — он должен умереть; для `WindowServer` кнопки «Снять» быть не должно. Если кликать в окне нечем — проверь то же юнит-тестом на функции, которая решает, показывать ли кнопку, и скажи об этом.

Правила:
- не коммить, не пушь; не создавай субагентов и сессий;
- никаких kill чужих процессов, кроме своих `sleep`;
- вне цели ничего не чини — впиши в debt;
- `cargo clippy -p rmon-app -- -D warnings` чистый; только сборки своего крейта, `CARGO_BUILD_JOBS=4`.

Последним в ответе — ровно этот блок:
RESULT
status: done | blocked | partial
changed: <файлы через запятую | none>
verify: <команды -> итог; RSS и CPU окна; что на скриншоте (путь); негативный контроль>
debt: <по строке на находку с приоритетом P0–P3 | none>
END
