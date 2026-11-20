//! Кольцо снимков за окно времени.

use std::collections::VecDeque;
use std::time::Duration;

use crate::model::{ProcessInfo, Snapshot};

/// История снимков. Старше окна — выбрасываются.
#[derive(Debug, Clone)]
pub struct History {
    window: Duration,
    snapshots: VecDeque<Snapshot>,
    slim: bool,
}

impl Default for History {
    fn default() -> Self {
        Self::new(Duration::from_secs(10 * 60))
    }
}

impl History {
    /// Пустая история с окном.
    pub fn new(window: Duration) -> Self {
        Self { window, snapshots: VecDeque::new(), slim: false }
    }

    /// История для долгоживущего процесса (окно): у всех снимков, кроме последнего,
    /// выбрасываются строки (имя, путь, аргументы). Анализу от старых снимков нужны
    /// только личность, память и CPU, а 800 процессов × сотни снимков со строками — это
    /// десятки мегабайт.
    pub fn slim(window: Duration) -> Self {
        Self { window, snapshots: VecDeque::new(), slim: true }
    }

    /// Добавить снимок и выбросить устаревшие.
    pub fn push(&mut self, snapshot: Snapshot) {
        let cutoff = snapshot.taken_at.saturating_sub(self.window.as_secs());
        if self.slim
            && let Some(prev) = self.snapshots.back_mut()
        {
            for p in &mut prev.processes {
                p.name = String::new();
                p.exe = None;
                p.args = Vec::new();
                p.app_group = String::new();
            }
            prev.processes.shrink_to_fit();
        }
        self.snapshots.push_back(snapshot);
        while self.snapshots.front().is_some_and(|s| s.taken_at < cutoff) {
            self.snapshots.pop_front();
        }
    }

    /// Последний снимок.
    pub fn latest(&self) -> Option<&Snapshot> {
        self.snapshots.back()
    }

    /// Все снимки, от старых к новым.
    pub fn snapshots(&self) -> impl DoubleEndedIterator<Item = &Snapshot> + ExactSizeIterator {
        self.snapshots.iter()
    }

    /// Сколько снимков.
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Пусто ли.
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// Ряд (время, память) одного процесса по личности (pid + started_at).
    pub fn series(&self, process: &ProcessInfo) -> Vec<(u64, u64)> {
        self.snapshots
            .iter()
            .filter_map(|s| {
                s.processes
                    .iter()
                    .find(|p| p.pid == process.pid && p.started_at == process.started_at)
                    .map(|p| (s.taken_at, p.reclaim()))
            })
            .collect()
    }

    /// Ряды всех процессов последнего снимка за один проход: (pid, started_at) → [(t, bytes)].
    pub fn all_series(&self) -> std::collections::HashMap<(i32, u64), Vec<(u64, u64)>> {
        let mut out: std::collections::HashMap<(i32, u64), Vec<(u64, u64)>> = std::collections::HashMap::new();
        for s in &self.snapshots {
            for p in &s.processes {
                out.entry((p.pid, p.started_at)).or_default().push((s.taken_at, p.reclaim()));
            }
        }
        out
    }
}

/// Наклон памяти, байт в минуту, методом наименьших квадратов. `None`, если точек мало.
pub fn slope_per_min(series: &[(u64, u64)]) -> Option<f64> {
    if series.len() < 3 {
        return None;
    }
    let t0 = series[0].0 as f64;
    let n = series.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for &(t, y) in series {
        let x = t as f64 - t0;
        let y = y as f64;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let denom = n * sxx - sx * sx;
    if denom.abs() < f64::EPSILON {
        return None;
    }
    Some((n * sxy - sx * sy) / denom * 60.0)
}
