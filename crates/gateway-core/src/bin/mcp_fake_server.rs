//! 测试用假 MCP stdio server：验证 mcp.rs 进程复用池的复用/崩溃/超时语义。
//!
//! 集成测试通过 `env!("CARGO_BIN_EXE_mcp_fake_server")` 拿到本二进制路径，
//! 由池作为子进程 spawn。工具：
//! - echo：回显 arguments.text
//! - pid：返回自身进程 id（判断是否复用了同一进程）
//! - count：返回本进程内累计调用次数（进程内状态延续）
//! - env：返回 FAKE_POOL_ENV 环境变量值（判断 env 是否参与连接键）
//! - crash：收到调用后立即退出（模拟崩溃，无响应）
//! - die_after_response：回包后立即退出（模拟「回包即崩」）
//! - slow：延迟 300ms 响应（测超时废弃）
//! - sleep：按 arguments.ms 延迟后响应

use std::io::{BufRead, Write};

fn respond(out: &mut impl Write, id: Option<serde_json::Value>, result: serde_json::Value) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","id":id,"result":result})
    );
}

fn respond_error(out: &mut impl Write, id: Option<serde_json::Value>, msg: String) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":msg}})
    );
}

fn main() {
    if std::env::args().nth(1).as_deref() != Some("--stdio-fake") {
        eprintln!("usage: mcp_fake_server --stdio-fake");
        std::process::exit(2);
    }

    use serde_json::Value;
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut count: i64 = 0;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue; // 非 JSON 行（噪音）忽略
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");

        match method {
            "initialize" => respond(
                &mut out,
                id,
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": {"name": "mcp-fake-server", "version": "1.0"},
                }),
            ),
            "notifications/initialized" => {}
            "tools/list" => respond(
                &mut out,
                id,
                serde_json::json!({"tools": [
                    {"name":"echo","description":"回显 text 参数","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}},
                    {"name":"pid","description":"返回当前进程 pid","inputSchema":{"type":"object"}},
                    {"name":"count","description":"返回本进程内累计调用次数","inputSchema":{"type":"object"}},
                    {"name":"env","description":"返回 FAKE_POOL_ENV 环境变量值","inputSchema":{"type":"object"}},
                    {"name":"crash","description":"收到调用后立即退出（模拟崩溃）","inputSchema":{"type":"object"}},
                    {"name":"die_after_response","description":"回包后立即退出","inputSchema":{"type":"object"}},
                    {"name":"slow","description":"延迟 300ms 响应（测超时）","inputSchema":{"type":"object"}},
                    {"name":"sleep","description":"按 ms 延迟后响应","inputSchema":{"type":"object","properties":{"ms":{"type":"integer"}}}},
                ]}),
            ),
            "tools/call" => {
                let name = msg
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let arguments = msg
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                match name {
                    "echo" => {
                        let text = arguments
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        respond(
                            &mut out,
                            id,
                            serde_json::json!({"content":[{"type":"text","text":text}]}),
                        );
                    }
                    "pid" => respond(
                        &mut out,
                        id,
                        serde_json::json!({"content":[{"type":"text","text":std::process::id().to_string()}]}),
                    ),
                    "count" => {
                        count += 1;
                        respond(
                            &mut out,
                            id,
                            serde_json::json!({"content":[{"type":"text","text":format!("count={count}")}]}),
                        );
                    }
                    "env" => {
                        let v = std::env::var("FAKE_POOL_ENV").unwrap_or_else(|_| "<unset>".into());
                        respond(
                            &mut out,
                            id,
                            serde_json::json!({"content":[{"type":"text","text":v}]}),
                        );
                    }
                    "crash" => std::process::exit(7),
                    "die_after_response" => {
                        respond(
                            &mut out,
                            id,
                            serde_json::json!({"content":[{"type":"text","text":"bye"}]}),
                        );
                        let _ = out.flush();
                        std::process::exit(8);
                    }
                    "slow" => {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        respond(
                            &mut out,
                            id,
                            serde_json::json!({"content":[{"type":"text","text":"slow-done"}]}),
                        );
                    }
                    "sleep" => {
                        let ms = arguments
                            .get("ms")
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                            .clamp(0, 10_000) as u64;
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                        respond(
                            &mut out,
                            id,
                            serde_json::json!({"content":[{"type":"text","text":"slept"}]}),
                        );
                    }
                    _ => respond_error(&mut out, id, format!("unknown tool: {name}")),
                }
            }
            _ => respond_error(&mut out, id, format!("unknown method: {method}")),
        }
    }
}
