# Resource Management — `rmon`

Лёгкий монитор оперативной памяти на Rust: macOS сейчас, Linux и Windows — за тем же трейтом.
Отвечает на вопросы, на которые Activity Monitor не отвечает: **кто съел память, растёт ли он,
нужен ли он ещё и можно ли его снять безопасно.**

- **Окно Resource Monitor** (Tauri): давление и его история, разбивка памяти, своп; приложения,
  процессы, подозреваемые (растёт, сирота, простаивает, брошенные копии) списком или карточками;
  панель «что это за процесс» — строка запуска, рабочий каталог, родители, потомки, график памяти;
  снятие через защитный слой.
- **Значок в строке меню**: цвет давления, «занято X ГБ», топ подозреваемых.
- **CLI `rmon`**: `status`, `ps`, `suggest`, `why <pid>`, `kill <pid>`, `config`, `mcp`; у всех `--json`.
- **MCP для Claude Code**: `rmon mcp` — Claude смотрит память и снимает процессы через тот же
  защитный слой (приложения владельца — только с его согласия). См. [docs/CLAUDE_CODE.md](docs/CLAUDE_CODE.md).

## Установка на Mac

```bash
scripts/install-macos.sh
```

Собирает приложение, кладёт его в `/Applications/Resource Monitor.app` и ссылку на CLI в
`~/.local/bin/rmon`. Нужны Rust (stable), Node 22+.

Установщики для других ОС собирает CI по тегу `v*` (`.github/workflows/release.yml`).

## Разработка

```bash
cargo test --workspace            # заодно пишет TS-типы и эталоны в apps/rmon-app/src/bindings
cd apps/rmon-app && npm ci && npm test && npx tauri dev
```

Спецификация и решения: [docs/SPEC.md](docs/SPEC.md).
