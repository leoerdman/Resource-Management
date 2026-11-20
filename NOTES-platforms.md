# Заметки полосы PLATFORMS (удалить перед финальным коммитом)
- repo не rustfmt-чистый (114 диффов) → fmt в CI не ставим.
- Linux footprint: RssAnon+RssShmem+VmSwap из status (дёшево, читается для всех процессов; smaps_rollup — обход таблиц страниц, 800 процессов раз в 2 с дорого).
- Linux: kernel threads (PF_KTHREAD) исключаем из списка.
- Windows: compressed — working set процесса "Memory Compression" (если удалось открыть), иначе 0. Счётчики VmCounters — нули.
