//! `/mcp` 元数据服务端到端测试：真实 HTTP + 安全中间件（鉴权/Host 校验）+
//! JSON-RPC 全流程。验证 Agent 侧按 dsh `streamable-http` 传输接入的可用性。

use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use gateway_core::vault;
use serde_json::{json, Value};

struct Fixture {
    port: u16,
    key: String,
    _keepalive: (tokio::sync::watch::Sender<bool>, tokio::task::JoinHandle<()>),
}

async fn fixture() -> Fixture {
    vault::testing::set_mock_default();

    let dir = std::env::temp_dir().join(format!(
        "jai-mcp-e2e-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let main_path = dir.join("main.db");
    let (db, main_path) = (Db::open(main_path.to_str().unwrap()).unwrap(), main_path);
    let (logs, _task) = gateway_core::store::logs::spawn_logger(main_path.to_str().unwrap())
        .unwrap();

    let now = store::now_ms();
    db.with(|c| {
        store::mcp_insert(
            c,
            &store::McpServerRow {
                id: "s1".into(),
                name: "registry-demo".into(),
                kind: "http".into(),
                command: None,
                args: None,
                url: Some("https://mcp.example.com/mcp".into()),
                env: None,
                enabled: true,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    // 网关密钥
    let key = "sk-jai-mcp-registry-test-00000000000000";
    db.with(|c| {
        store::gw_key_rotate(c, key, Some("test"))?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let ctx = GatewayCtx::new(db.clone(), logs);
    let app = server::build_router(ctx);
    let (listener, port) = server::bind_with_fallback("127.0.0.1", 0).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let guard = tokio::spawn(async move {
        let _ = server::run_until_shutdown(listener, app, stop_rx).await;
    });

    Fixture {
        port,
        key: key.to_string(),
        _keepalive: (stop_tx, guard),
    }
}

impl Fixture {
    async fn rpc(&self, body: Value, auth: bool) -> (u16, Value) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/mcp", self.port);
        let mut req = client
            .post(&url)
            .header("host", format!("127.0.0.1:{}", self.port))
            .json(&body);
        if auth {
            req = req.bearer_auth(&self.key);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }
}

#[tokio::test]
async fn mcp_unauthenticated_rejected() {
    let fx = fixture().await;
    let (status, _) = fx
        .rpc(json!({"jsonrpc":"2.0","method":"tools/list","params":{},"id":1}), false)
        .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn mcp_full_flow_with_auth() {
    let fx = fixture().await;

    // initialize
    let (status, resp) = fx
        .rpc(json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}), true)
        .await;
    assert_eq!(status, 200);
    assert_eq!(resp["result"]["serverInfo"]["name"], "jai-gateway-registry");

    // notifications/initialized（无 id）→ 202
    let (status, body) = fx
        .rpc(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), true)
        .await;
    assert_eq!(status, 202);
    assert!(body.is_null());

    // tools/list
    let (_, resp) = fx
        .rpc(json!({"jsonrpc":"2.0","method":"tools/list","params":{},"id":2}), true)
        .await;
    assert_eq!(resp["result"]["tools"].as_array().unwrap().len(), 5);

    // tools/call: list_mcp_servers
    let (_, resp) = fx
        .rpc(
            json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"list_mcp_servers","arguments":{}},"id":3}),
            true,
        )
        .await;
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let payload: Value = serde_json::from_str(text).unwrap();
    let servers = payload["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "registry-demo");

    // tools/call: get_mcp_server_detail（http 类型 → connect.url）
    let (_, resp) = fx
        .rpc(
            json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"get_mcp_server_detail","arguments":{"name":"registry-demo"}},"id":4}),
            true,
        )
        .await;
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let payload: Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["connect"]["url"], "https://mcp.example.com/mcp");
}
