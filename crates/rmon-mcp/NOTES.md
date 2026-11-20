# rmon-mcp — заметки по ходу работы

- rmcp 3.11.0 (server + macros + transport-io), tokio current_thread.
- Ядро: добавить journal::read_guard_recent / last_guard_zone (хвост файла с конца: events.jsonl 1,7 МБ),
  fmt::bytes_delta; починить fmt::bytes_per_min для отрицательных (терял знак), guard_text для ev=kill (нет name — есть kind/memMb).
- Цель kill: только MonitorHandle::kill; процесс младше последнего снимка (≤5 с) — «не в снимке, повторите».
