//! Сквозная проверка протокола: бинарь `rmon-mcp` на stdio, запрос за запросом.
//!
//! initialize → tools/list → memory_overview → kill_process pid 1 (отказ) → неизвестный
//! инструмент (ошибка) → `pid: "abc"` (ошибка) → ping: сервер переживает ошибки и отвечает дальше.

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Server {
    fn start(scratch: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rmon-mcp"))
            // Отказы снятия пишутся в журнал действий — не в журнал владельца.
            .env("RMON_ACTIONS_LOG", scratch.join("actions.jsonl"))
            .env("RMON_GUARD_LOG", scratch.join("events.jsonl"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("запуск rmon-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, stdin, lines, next_id: 1 }
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Запрос и ответ на него (по id).
    fn call(&mut self, method: &str, params: Option<Value>) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.send(&msg);
        loop {
            let line = self.lines.recv_timeout(Duration::from_secs(20)).unwrap_or_else(|_| panic!("нет ответа на {method}"));
            let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout не JSON ({e}): {line}"));
            if v["id"] == json!(id) {
                assert_eq!(v["jsonrpc"], "2.0");
                return v;
            }
        }
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        self.call("tools/call", Some(json!({"name": name, "arguments": args})))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

/// Ошибка вызова: либо JSON-RPC error, либо результат с isError (так rmcp отдаёт ошибки разбора аргументов).
fn assert_failed(reply: &Value, what: &str) {
    let is_rpc_error = reply["error"]["code"].is_i64();
    let is_tool_error = reply["result"]["isError"] == json!(true);
    assert!(is_rpc_error || is_tool_error, "{what}: ожидалась ошибка, пришло {reply}");
}

#[test]
fn serves_protocol_and_survives_errors() {
    let scratch = std::env::temp_dir().join(format!("rmon-mcp-it-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut s = Server::start(&scratch);

    let init = s.call(
        "initialize",
        Some(json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "it", "version": "0"}})),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "rmon", "{init}");
    assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");
    assert!(init["result"]["instructions"].as_str().unwrap().contains("confirm=true"));
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let list = s.call("tools/list", None);
    let tools = list["result"]["tools"].as_array().expect("tools");
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["guard_events", "kill_process", "list_processes", "memory_overview", "process_details", "suggest_cleanup"]);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert_eq!(t["outputSchema"]["type"], "object", "{t}");
    }
    let kill = tools.iter().find(|t| t["name"] == "kill_process").unwrap();
    let desc = kill["description"].as_str().unwrap();
    assert!(desc.contains("ask the user before calling with confirm=true"), "{desc}");
    assert_eq!(kill["inputSchema"]["required"], json!(["pid", "reason"]));

    let ov = s.tool("memory_overview", json!({}));
    let r = &ov["result"];
    assert_eq!(r["isError"], json!(false), "{ov}");
    assert!(text(r).starts_with("Память: занято"), "{}", text(r));
    assert!(r["structuredContent"]["memory"]["total"].as_u64().unwrap() > 0, "{ov}");
    assert!(r["structuredContent"]["top_apps"].as_array().is_some_and(|a| !a.is_empty()), "{ov}");
    assert!(r["structuredContent"]["guard"].is_null(), "журнала сторожа нет — guard null: {ov}");

    let k = s.tool("kill_process", json!({"pid": 1, "reason": "integration test"}));
    let r = &k["result"];
    assert_eq!(r["isError"], json!(true), "{k}");
    assert_eq!(r["structuredContent"]["status"], "refused_protected", "{k}");
    assert!(text(r).contains("запрещено"), "{}", text(r));

    let unknown = s.tool("no_such_tool", json!({}));
    assert_failed(&unknown, "неизвестный инструмент");

    let bad = s.tool("kill_process", json!({"pid": "abc", "reason": "x"}));
    assert_failed(&bad, "pid строкой");
    let bad = s.tool("list_processes", json!({"limit": 0}));
    assert_failed(&bad, "limit 0");

    let ge = s.tool("guard_events", json!({"limit": 5}));
    assert_eq!(ge["result"]["structuredContent"]["exists"], json!(false), "{ge}");
    assert_eq!(ge["result"]["isError"], json!(false), "нет журнала — не ошибка: {ge}");

    let ping = s.call("ping", None);
    assert!(ping["result"].is_object(), "{ping}");
    let _ = std::fs::remove_dir_all(&scratch);
}
