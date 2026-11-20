//! rmon-mcp — MCP-сервер (stdio) поверх `rmon-core`: Claude Code видит память и процессы,
//! получает предложения, что снять, и снимает процессы через защитный слой ядра.
//!
//! Сбор — фоновый монитор ядра ([`rmon_core::monitor::spawn`] с
//! [`MonitorConfig::background`]: снимок раз в 5 с, окно 15 мин). Снятие — только
//! [`MonitorHandle::kill`] с `requested_by = Mcp`. stdout — протокол, логи — только stderr.

pub mod args;
pub mod view;

use std::sync::Arc;
use std::time::Duration;

use rmcp::handler::server::common::schema_for_output;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use rmon_core::collect::unix_now;
use rmon_core::kill::{KillPolicy, KillRequest, Requester, policy_for};
use rmon_core::monitor::{Dashboard, MonitorConfig, MonitorHandle};
use serde::Serialize;

use crate::args::{CleanupArgs, DetailsArgs, GuardArgs, KillArgs, ListArgs, Validate};
use crate::view::{CleanupOut, DetailsOut, GuardOut, KillOut, ListOut, OverviewOut};

/// Сколько ждать первого снимка после запуска сервера.
const FIRST_SNAPSHOT_WAIT: Duration = Duration::from_secs(5);

/// Ответ инструмента: текст человеку + структура по схеме.
fn reply<T: Serialize>(text: String, data: &T, is_error: bool) -> Result<CallToolResult, ErrorData> {
    let value = serde_json::to_value(data).map_err(|e| ErrorData::internal_error(format!("serialize: {e}"), None))?;
    let content = vec![ContentBlock::text(text)];
    let mut r = if is_error { CallToolResult::error(content) } else { CallToolResult::success(content) };
    r.structured_content = Some(value);
    Ok(r)
}

/// MCP-сервер rmon.
#[derive(Clone)]
pub struct RmonServer {
    monitor: MonitorHandle,
}

impl RmonServer {
    /// Сервер поверх запущенного монитора.
    pub fn new(monitor: MonitorHandle) -> Self {
        Self { monitor }
    }

    /// Последняя сводка; сразу после запуска ждёт первый снимок.
    async fn dashboard(&self) -> Result<Arc<Dashboard>, ErrorData> {
        let step = Duration::from_millis(100);
        let mut waited = Duration::ZERO;
        loop {
            if let Some(d) = self.monitor.dashboard() {
                return Ok(d);
            }
            if waited >= FIRST_SNAPSHOT_WAIT {
                return Err(ErrorData::internal_error(
                    "первый снимок памяти ещё не готов — повторите через несколько секунд",
                    None,
                ));
            }
            tokio::time::sleep(step).await;
            waited += step;
        }
    }

    /// Жив ли процесс сейчас (свежее чтение платформы).
    fn alive_now(pid: i32) -> bool {
        rmon_core::platform::current().ok().and_then(|mut p| p.alive(pid).ok()).unwrap_or(false)
    }

    /// Ответ `kill_process` для процесса не из последнего снимка: свежее чтение платформы
    /// отличает только что запущенный от нечитаемого (системного, чужого) и от несуществующего.
    fn kill_missing(pid: i32) -> KillOut {
        let mut plat = rmon_core::platform::current().ok();
        let fresh = plat.as_mut().and_then(|p| p.process(pid).ok().flatten());
        let alive = fresh.is_some() || plat.as_mut().and_then(|p| p.alive(pid).ok()).unwrap_or(false);
        let policy = if pid <= rmon_core::platform::LAST_KERNEL_PID {
            Some(KillPolicy::Protected("ядро системы".into()))
        } else {
            fresh.as_ref().map(policy_for)
        };
        view::kill_missing(pid, fresh.as_ref().map(|p| p.name.as_str()), policy.as_ref(), alive)
    }
}

#[tool_router]
impl RmonServer {
    #[tool(
        description = "Memory overview of the Mac: pressure, used/total, app/wired/compressed/cached/free, swap, \
        pressure and swap trend over the last 15 minutes, top 10 applications by memory, top 5 cleanup suspects, \
        and the current zone of the rmon memory guard (formerly memory-guard). Call this first when the user asks about memory.",
        annotations(title = "Memory overview", read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<OverviewOut>()
    )]
    async fn memory_overview(&self) -> Result<CallToolResult, ErrorData> {
        let d = self.dashboard().await?;
        let zone = rmon_core::journal::last_guard_zone();
        let (out, text) = view::overview(&d, zone.as_ref(), unix_now());
        reply(text, &out, false)
    }

    #[tool(
        description = "List processes with memory (footprint), CPU, memory growth rate and kill policy. \
        sort: mem | cpu | growth | pid (default mem); limit (default 25, max 500); filter: substring of name, \
        app, path or command line (a number also matches the pid); group_by_app: sum processes per application.",
        annotations(title = "List processes", read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<ListOut>()
    )]
    async fn list_processes(&self, Parameters(a): Parameters<ListArgs>) -> Result<CallToolResult, ErrorData> {
        a.validate()?;
        let d = self.dashboard().await?;
        let (out, text) = view::list(&d, &a);
        reply(text, &out, false)
    }

    #[tool(
        description = "Details of one process: memory, CPU, command line, working directory, parent chain up to \
        launchd, descendants with their total memory, memory series over the last 15 minutes, kill policy and \
        its reason, and the suspect verdict if any.",
        annotations(title = "Process details", read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<DetailsOut>()
    )]
    async fn process_details(&self, Parameters(a): Parameters<DetailsArgs>) -> Result<CallToolResult, ErrorData> {
        a.validate()?;
        let d = self.dashboard().await?;
        let Some(det) = self.monitor.details(a.pid, None) else {
            let text = view::details_missing(a.pid, Self::alive_now(a.pid));
            return Ok(CallToolResult::error(vec![ContentBlock::text(text)]));
        };
        let full = self.monitor.target(a.pid, Some(det.row.started_at));
        let suspect = d.suspects.iter().find(|s| s.pid == a.pid && s.started_at == det.row.started_at);
        let (out, text) = view::details(&det, full.as_ref(), suspect);
        reply(text, &out, false)
    }

    #[tool(
        description = "Cleanup suggestions: suspects (big, growing, orphaned, idle, duplicate, zombie processes) \
        with a reason, memory to be freed and kill policy, plus totals: how much can be freed without asking the \
        user (policy free) and how much only with the user's consent (policy confirm). min_score default 30.",
        annotations(title = "Suggest cleanup", read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<CleanupOut>()
    )]
    async fn suggest_cleanup(&self, Parameters(a): Parameters<CleanupArgs>) -> Result<CallToolResult, ErrorData> {
        a.validate()?;
        let d = self.dashboard().await?;
        let (out, text) = view::cleanup(&d, a.min_score);
        reply(text, &out, false)
    }

    #[tool(
        description = "Terminate a process through rmon's safety layer: identity check, SIGTERM, 5 s grace, then \
        SIGKILL (hard=true: SIGKILL right away); every action and refusal is journaled. Policy: protected processes \
        (system, launchd, WindowServer, rmon itself, other users) are ALWAYS refused. For Confirm-policy processes \
        (user apps: browsers, Claude, M Code, Telegram, editors, claude CLI sessions, system utilities) ask the user \
        before calling with confirm=true; never set confirm=true on your own initiative. Free processes \
        (dev runs and the like) need no confirmation. dry_run=true only reports what would happen.",
        annotations(title = "Kill process", read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = schema_for_output::<KillOut>()
    )]
    async fn kill_process(&self, Parameters(a): Parameters<KillArgs>) -> Result<CallToolResult, ErrorData> {
        a.validate()?;
        self.dashboard().await?;
        let Some(target) = self.monitor.target(a.pid, None) else {
            let out = Self::kill_missing(a.pid);
            return reply(out.message.clone(), &out, true);
        };
        let policy = policy_for(&target);
        let req = KillRequest {
            signal_term_grace: if a.hard { Duration::ZERO } else { KillRequest::default().signal_term_grace },
            confirm: a.confirm,
            requested_by: Requester::Mcp,
            reason: a.reason.trim().to_owned(),
            dry_run: a.dry_run,
        };
        let handle = self.monitor.clone();
        let (pid, started_at) = (target.pid, target.started_at);
        // Снятие ждёт до 7 с — не держим им единственный поток рантайма.
        let res = tokio::task::spawn_blocking(move || handle.kill(pid, Some(started_at), req))
            .await
            .map_err(|e| ErrorData::internal_error(format!("kill task: {e}"), None))?;
        let out = match res {
            Ok(o) => view::kill_ok(&o, &policy),
            Err(e) => view::kill_err(pid, Some(&target.name), Some(&policy), &e),
        };
        let is_error = out.is_error();
        reply(out.message.clone(), &out, is_error)
    }

    #[tool(
        description = "Recent events of the rmon memory guard (formerly memory-guard) (zone changes green/yellow/red, its own kills, \
        kill-rate limits), newest first. limit default 20. Missing journal is not an error: empty list.",
        annotations(title = "memory-guard events", read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<GuardOut>()
    )]
    async fn guard_events(&self, Parameters(a): Parameters<GuardArgs>) -> Result<CallToolResult, ErrorData> {
        a.validate()?;
        let entries = rmon_core::journal::read_guard_recent(a.limit as usize);
        let (out, text) = view::guard(&entries, &rmon_core::journal::guard_path(), unix_now());
        reply(text, &out, false)
    }
}

#[tool_handler(
    name = "rmon",
    instructions = "rmon: memory and process monitor of the user's Mac (24 GB RAM, many AI agents). \
Start with memory_overview; use suggest_cleanup for what to terminate and how much it frees; \
process_details explains a single process (parent chain, children, cwd, memory series). \
kill_process goes through a safety layer: protected processes are always refused; for Confirm-policy \
processes (user apps and their helpers) ask the user before calling with confirm=true. \
Human-readable text is in Russian — relay it to the user as is."
)]
impl ServerHandler for RmonServer {}

/// Запустить сервер на stdin/stdout и работать, пока клиент не закроет stdin.
pub async fn serve_stdio() -> anyhow::Result<()> {
    let monitor = rmon_core::monitor::spawn(MonitorConfig::background(), |_| {})?;
    let service = RmonServer::new(monitor).serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// Синхронная обёртка над [`serve_stdio`]: свой однопоточный рантайм tokio. Для `rmon mcp`.
pub fn serve() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(serve_stdio())
}
