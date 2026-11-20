//! Сборщик снимков: платформа → [`Snapshot`], CPU по разнице двух снимков.

use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::model::{ProcessInfo, Snapshot};
use crate::platform::{self, Platform};

/// Делает снимки. Держит прошлое процессорное время, чтобы считать `cpu_pct`.
pub struct Collector {
    platform: Box<dyn Platform>,
    /// (pid, started_at) → (cpu_time_ns, когда).
    last: HashMap<(i32, u64), u64>,
    last_at: Option<Instant>,
}

impl Collector {
    /// Сборщик для текущей ОС.
    pub fn new() -> Result<Self> {
        Ok(Self::with_platform(platform::current()?))
    }

    /// Сборщик поверх своей платформы (тесты, другие ОС).
    pub fn with_platform(platform: Box<dyn Platform>) -> Self {
        Self { platform, last: HashMap::new(), last_at: None }
    }

    /// Снимок системы и всех процессов.
    pub fn snapshot(&mut self) -> Result<Snapshot> {
        let system = self.platform.system_memory()?;
        let samples = self.platform.processes()?;
        let now = Instant::now();
        let elapsed_ns = self.last_at.map(|t| now.duration_since(t).as_nanos() as f64);

        let mut next = HashMap::with_capacity(samples.len());
        let processes = samples
            .into_iter()
            .map(|s| {
                let key = (s.info.pid, s.info.started_at);
                let mut info = s.info;
                if let (Some(el), Some(prev)) = (elapsed_ns, self.last.get(&key))
                    && el > 0.0
                {
                    info.cpu_pct = (s.cpu_time_ns.saturating_sub(*prev) as f64 * 100.0 / el) as f32;
                }
                next.insert(key, s.cpu_time_ns);
                info
            })
            .collect();
        self.last = next;
        self.last_at = Some(now);
        Ok(Snapshot { taken_at: unix_now(), system, processes })
    }

    /// Свежий вид одного процесса — для сверки личности перед снятием.
    pub fn process(&mut self, pid: i32) -> Result<Option<ProcessInfo>> {
        self.platform.process(pid)
    }

    /// Доступ к платформе (снятие процессов).
    pub fn platform_mut(&mut self) -> &mut dyn Platform {
        self.platform.as_mut()
    }
}

/// Текущее время, unix-секунды.
pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
