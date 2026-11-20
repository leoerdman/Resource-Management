//! Модель данных — контракт для CLI, MCP-сервера и окна (docs/SPEC.md).

use serde::{Deserialize, Serialize};

/// Кибибайт.
pub const KIB: u64 = 1024;
/// Мебибайт.
pub const MIB: u64 = 1024 * KIB;
/// Гибибайт.
pub const GIB: u64 = 1024 * MIB;

/// Давление памяти из `kern.memorystatus_vm_pressure_level` (1 / 2 / 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pressure {
    /// Зелёная зона.
    #[default]
    Normal,
    /// Жёлтая зона.
    Warning,
    /// Красная зона: система сжимает и свопит всё, что может.
    Critical,
}

impl Pressure {
    /// Уровень ядра: 1 — норма, 2 — предупреждение, 4 — критично.
    pub fn from_level(level: i32) -> Self {
        match level {
            1 => Self::Normal,
            2 => Self::Warning,
            l if l >= 4 => Self::Critical,
            l if l > 2 => Self::Warning,
            _ => Self::Normal,
        }
    }

    /// Слово для человека.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "нормальное",
            Self::Warning => "повышенное",
            Self::Critical => "критическое",
        }
    }
}

/// Состояние процесса (`pbi_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcState {
    /// SRUN.
    Running,
    /// SSLEEP или SIDL.
    Sleeping,
    /// SSTOP.
    Stopped,
    /// SZOMB: процесс умер, но не пожат родителем.
    Zombie,
    /// Не разобрали или нет прав.
    #[default]
    Unknown,
}

impl ProcState {
    /// Слово для человека.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "работает",
            Self::Sleeping => "спит",
            Self::Stopped => "остановлен",
            Self::Zombie => "зомби",
            Self::Unknown => "неизвестно",
        }
    }
}

/// Накопительные счётчики ядра — разница между снимками показывает активность.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VmCounters {
    /// Страниц подкачано с диска.
    pub pageins: u64,
    /// Страниц вытеснено на диск.
    pub pageouts: u64,
    /// Страниц поднято из свопа.
    pub swapins: u64,
    /// Страниц отправлено в своп.
    pub swapouts: u64,
    /// Сжатий страниц.
    pub compressions: u64,
    /// Распаковок страниц.
    pub decompressions: u64,
}

/// Память системы целиком. `used = app + wired + compressed`, как «Memory Used» в Activity Monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SystemMemory {
    /// `hw.memsize`.
    pub total: u64,
    /// Занято: `app + wired + compressed`.
    pub used: u64,
    /// Память приложений: `(internal - purgeable) * page`.
    pub app: u64,
    /// Wired: не выгружается.
    pub wired: u64,
    /// Сжато компрессором: `compressor_page_count * page`.
    pub compressed: u64,
    /// Кэш файлов: `(external + purgeable) * page`.
    pub cached_files: u64,
    /// Свободно: `free_count * page`.
    pub free: u64,
    /// `vm.swapusage` total.
    pub swap_total: u64,
    /// `vm.swapusage` used.
    pub swap_used: u64,
    /// Давление памяти.
    pub pressure: Pressure,
    /// `kern.memorystatus_level` — сколько процентов ещё доступно.
    pub available_pct: u8,
    /// Накопительные счётчики.
    pub counters: VmCounters,
}

impl SystemMemory {
    /// Доля занятой памяти, проценты.
    pub fn used_pct(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.used as f64 * 100.0 / self.total as f64
        }
    }

    /// Свободно с учётом кэша: то, что система отдаст под новые задачи.
    pub fn reclaimable(&self) -> u64 {
        self.free.saturating_add(self.cached_files)
    }
}

/// Один процесс в снимке.
///
/// `(pid, started_at)` — личность процесса: pid переиспользуется, дата запуска — нет.
/// `args` — вторая половина личности: `exec` на месте может оказаться с другими аргументами.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Идентификатор процесса.
    pub pid: i32,
    /// Идентификатор родителя.
    pub ppid: i32,
    /// Короткое имя (`proc_name`, иначе `pbi_name`).
    pub name: String,
    /// Путь к исполняемому файлу (`proc_pidpath`); `None`, если нет прав.
    pub exe: Option<String>,
    /// Аргументы из `KERN_PROCARGS2`, без окружения и без пути запуска.
    pub args: Vec<String>,
    /// Владелец процесса.
    pub uid: u32,
    /// `ri_phys_footprint` — «Память» в Activity Monitor; `None`, если нет прав.
    pub footprint: Option<u64>,
    /// `ri_resident_size` — резидентая память.
    pub rss: u64,
    /// Загрузка CPU по разнице двух снимков; в первом снимке 0.
    pub cpu_pct: f32,
    /// Число потоков.
    pub threads: u32,
    /// Состояние.
    pub state: ProcState,
    /// Время запуска, unix-секунды.
    pub started_at: u64,
    /// Имя приложения-владельца: «Google Chrome», «M Code Desktop», иначе имя процесса.
    pub app_group: String,
}

impl ProcessInfo {
    /// Сколько освободится при снятии: footprint, иначе rss.
    pub fn reclaim(&self) -> u64 {
        self.footprint.unwrap_or(self.rss)
    }

    /// Совпадает ли личность: pid, время запуска и аргументы.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.pid == other.pid && self.started_at == other.started_at && self.args == other.args
    }

    /// Строка запуска — для ключа дублей и для человека.
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            match &self.exe {
                Some(exe) => exe.clone(),
                None => self.name.clone(),
            }
        } else {
            self.args.join(" ")
        }
    }
}

/// Снимок системы и процессов в один момент.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Время снимка, unix-секунды.
    pub taken_at: u64,
    /// Память системы.
    pub system: SystemMemory,
    /// Все процессы, какие удалось прочитать.
    pub processes: Vec<ProcessInfo>,
}

impl Snapshot {
    /// Процесс по pid.
    pub fn process(&self, pid: i32) -> Option<&ProcessInfo> {
        self.processes.iter().find(|p| p.pid == pid)
    }

    /// Память, которую вернут снятые процессы из списка.
    pub fn reclaimed(&self, pids: &[i32]) -> u64 {
        self.processes
            .iter()
            .filter(|p| pids.contains(&p.pid))
            .map(ProcessInfo::reclaim)
            .sum()
    }
}

/// Приложение целиком: все его процессы одной суммой.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AppGroup {
    /// Имя приложения.
    pub name: String,
    /// Процессы приложения.
    pub pids: Vec<i32>,
    /// Суммарный footprint (иначе rss).
    pub footprint: u64,
    /// Сколько процессов.
    pub count: usize,
}
