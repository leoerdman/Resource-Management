//! `rmon-mcp` — MCP-сервер rmon на stdio (то же, что `rmon mcp`).

fn main() -> anyhow::Result<()> {
    rmon_mcp::serve()
}
