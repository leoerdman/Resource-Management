//! rmon-core — ядро монитора памяти macOS.
//!
//! Слой сбора (`platform`) даёт сырые цифры, `collect` превращает их в [`Snapshot`],
//! `history` хранит окно снимков, `analyze` ищет подозреваемых, `kill` снимает процессы
//! через защитный слой. Всё выше трейта [`platform::Platform`] платформонезависимо.
//!
//! Публичный API — контракт для полос MCP и APP (docs/SPEC.md): все типы сериализуются
//! в JSON без потерь.

pub mod analyze;
pub mod collect;
pub mod config;
pub mod contract;
pub mod error;
pub mod fmt;
pub mod group;
pub mod history;
pub mod journal;
pub mod kill;
pub mod model;
pub mod monitor;
pub mod platform;
pub mod policy;

pub use error::{Error, Result};
pub use model::{
    AppGroup, Pressure, ProcState, ProcessInfo, Snapshot, SystemMemory, VmCounters,
};
