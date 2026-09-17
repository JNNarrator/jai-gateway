//! Skill 全生命周期回归（真实 HTTP + JSON-RPC，断言 `isError` 真值）。
//!
//! 固化 2026-09-17 对运行中实例的手工验证结论，并钉死三处修复：
//! - bug 15：技能名可任意（中文/空格/斜杠/超长/重名），但**广告出去的工具名必须合法**
//!   （`[A-Za-z0-9_-]{1,64}`），否则严格校验的客户端可能拒绝整份 tools/list；
//! - bug 16：`get_skill_detail` 与投递口径一致——未启用技能不返回全文；
//! - bug 17：台账类工具失败必须 `isError=true`（不得把失败拉平成成功）。
//!
//! 另覆盖：合法原名保持向后兼容（仍为 `skill__<原名>`）、全文逐字节无损投递、
//! 32KB 截断落在 UTF-8 字符边界。

use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

struct Fixture {
    port: u16,
    key: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

/// 32KB 边界夹具：首字节 ASCII + 大量中文（3 字节/字），使第 32768 字节落在字符中间。
const BIG_CONTENT_A: &str = "A";
const NORMAL_CONTENT: &str =
    "步骤一：读取 `config.json`\n步骤二：执行 \"pnpm test\"（引号与反引号都要保留）\n```bash\necho \"hello 世界\"\n```\n结尾标记 EOF-MARKER-42\n";
const DISABLED_SECRET: &str = "SECRET-IN-DISABLED-9F3A";

async fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "jai-skill-lifecycle-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let main_path = dir.join("main.db");
    let db = Db::open(main_path.to_str().unwrap()).unwrap();
    let (logs, _task) =
        gateway_core::store::logs::spawn_logger(main_path.to_str().unwrap()).unwrap();
    let key = "sk-jai-lifecycle-test".to_string();

    let now = store::now_ms();
    let big = format!("{BIG_CONTENT_A}{}", "中".repeat(20000)); // 1 + 60000 字节
    db.with(|c| {
        store::gw_key_rotate(c, &key, Some("skill-lifecycle-test"))?;
        for (id, name, desc, content, enabled) in [
            (
                "s-normal",
                "code-review",
                "合法原名：应保持 skill__code-review",
                NORMAL_CONTENT,
                true,
            ),
            (
                "s-weird",
                "代码 评审/甲",
                "名字含非法字符：工具名必须被安全化",
                "内容-weird",
                true,
            ),
            (
                "s-disabled",
                "disabled-skill",
                "未启用：不得投递、不得返回全文",
                DISABLED_SECRET,
                false,
            ),
            (
                "s-big",
                "big-skill",
                "超 32KB：应截断且落在字符边界",
                &big,
                true,
            ),
        ] {
            store::skill_insert(
                c,
                &store::SkillRow {
                    id: id.into(),
                    name: name.into(),
                    description: desc.into(),
                    content: content.into(),
                    enabled,
                    created_at: now,
                    updated_at: now,
                },
            )?;
        }
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
        key,
        _keepalive: (stop_tx, guard),
    }
}

impl Fixture {
    async fn rpc(&self, body: Value) -> Value {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/mcp", self.port);
        let resp = client
            .post(&url)
            .header("host", format!("127.0.0.1:{}", self.port))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        resp.json().await.unwrap_or(Value::Null)
    }

    async fn call(&self, name: &str, args: Value) -> Value {
        self.rpc(json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args}
        }))
        .await["result"]
            .clone()
    }

    async fn tool_names(&self) -> Vec<String> {
        self.rpc(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
            .await["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }
}

fn text_of(result: &Value) -> String {
    result["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| b["text"].as_str().unwrap_or(""))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// MCP 名契约：`[A-Za-z0-9_-]{1,64}`。
fn name_is_legal(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// bug 15：所有广告出去的工具名（技能 + 代理）都必须合法；非法技能名不得泄漏到工具名。
#[tokio::test]
async fn advertised_tool_names_are_spec_legal() {
    let fx = fixture().await;
    let names = fx.tool_names().await;
    for n in &names {
        assert!(name_is_legal(n), "工具名违反 MCP 契约: {n:?}");
    }
    // 非法原名不得原样出现在工具名里
    assert!(
        !names
            .iter()
            .any(|n| n.contains("代码 评审") || n.contains('/')),
        "非法技能名泄漏到工具名: {names:?}"
    );
}

/// bug 15 + 向后兼容：合法原名保持 `skill__<原名>`；非法名编码后仍可投递，且真实名字在 description 里。
#[tokio::test]
async fn legal_name_kept_and_illegal_name_encoded() {
    let fx = fixture().await;
    let names = fx.tool_names().await;
    assert!(
        names.iter().any(|n| n == "skill__code-review"),
        "合法原名应保持原样（向后兼容）: {names:?}"
    );
    let weird = names
        .iter()
        .find(|n| {
            n.starts_with("skill__") && *n != "skill__code-review" && *n != "skill__big-skill"
        })
        .unwrap_or_else(|| panic!("应存在编码后的怪名技能工具: {names:?}"));
    assert!(name_is_legal(weird), "编码后仍非法: {weird}");

    // 描述里必须能看到真实技能名（模型仍可读）
    let tools = fx
        .rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .await["result"]["tools"]
        .clone();
    let desc = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == weird.as_str())
        .unwrap()["description"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(desc.contains("代码 评审/甲"), "描述应带真实名: {desc}");

    // 用编码后的工具名投递
    let r = fx.call(weird, json!({})).await;
    assert_eq!(r["isError"], false, "编码名应可正常投递: {r}");
    assert!(text_of(&r).contains("内容-weird"));
}

/// 全文逐字节无损 + 未启用/未知技能的工具级失败语义。
#[tokio::test]
async fn delivery_is_lossless_and_disabled_is_rejected() {
    let fx = fixture().await;

    let ok = fx.call("skill__code-review", json!({})).await;
    assert_eq!(ok["isError"], false);
    let payload: Value = serde_json::from_str(&text_of(&ok)).unwrap();
    assert_eq!(
        payload["content"], NORMAL_CONTENT,
        "投递内容必须与库内逐字节一致（引号/反引号/中文/换行都不许动）"
    );
    assert!(text_of(&ok).contains("EOF-MARKER-42"), "尾部不得被截断");

    let disabled = fx.call("skill__disabled-skill", json!({})).await;
    assert_eq!(disabled["isError"], true, "未启用技能投递必须 isError=true");
    assert!(text_of(&disabled).contains("未启用"), "{disabled}");

    let unknown = fx.call("skill__nope", json!({})).await;
    assert_eq!(unknown["isError"], true);
    assert!(text_of(&unknown).contains("未找到"), "{unknown}");
}

/// bug 16：`get_skill_detail` 与投递口径一致（未启用 → 拒绝），且必须 isError=true。
#[tokio::test]
async fn skill_detail_follows_enabled_gate() {
    let fx = fixture().await;

    let ok = fx
        .call("get_skill_detail", json!({"name": "code-review"}))
        .await;
    assert_eq!(ok["isError"], false);
    assert!(text_of(&ok).contains("EOF-MARKER-42") || text_of(&ok).contains(NORMAL_CONTENT));

    let disabled = fx
        .call("get_skill_detail", json!({"name": "disabled-skill"}))
        .await;
    assert_eq!(
        disabled["isError"], true,
        "未启用技能不得通过台账读出全文（bug 16）: {disabled}"
    );
    assert!(
        !text_of(&disabled).contains(DISABLED_SECRET),
        "未启用技能内容泄漏: {disabled}"
    );
    assert!(text_of(&disabled).contains("未启用"), "{disabled}");

    let unknown = fx.call("get_skill_detail", json!({"name": "不存在"})).await;
    assert_eq!(unknown["isError"], true, "bug 17：未找到必须 isError=true");

    let missing = fx.call("get_skill_detail", json!({})).await;
    assert_eq!(missing["isError"], true, "缺参数必须 isError=true");
    assert!(text_of(&missing).contains("缺少参数"), "{missing}");
}

/// bug 17：台账类工具（server 明细 / 工具 schema）失败同样必须 isError=true。
#[tokio::test]
async fn ledger_tools_fail_loudly() {
    let fx = fixture().await;
    for (tool, args) in [
        ("get_mcp_server_detail", json!({"name": "不存在的 server"})),
        ("get_tool_schemas", json!({"name": "不存在的 server"})),
        ("get_mcp_server_detail", json!({})),
    ] {
        let r = fx.call(tool, args.clone()).await;
        assert_eq!(r["isError"], true, "{tool} 失败未冒泡: {r} (args={args})");
    }
    // 成功路径仍是 isError=false
    let ok = fx.call("list_skills", json!({})).await;
    assert_eq!(ok["isError"], false, "{ok}");
}

/// 超 32KB：截断提示 + 截断处不切多字节字符 + 保留正确前缀。
#[tokio::test]
async fn oversized_skill_truncates_on_char_boundary() {
    let fx = fixture().await;
    let r = fx.call("skill__big-skill", json!({})).await;
    assert_eq!(r["isError"], false);
    let payload: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(payload["truncated"], true);

    let content = payload["content"].as_str().unwrap();
    assert!(
        content.contains("已截断"),
        "应注明截断: {}",
        &content[..80.min(content.len())]
    );
    let cut = content.split("\n\n……").next().unwrap();
    assert!(cut.len() <= 32 * 1024, "截断后仍超上限: {}", cut.len());
    assert!(
        cut.is_char_boundary(cut.len()),
        "截断落在多字节字符中间（会出现乱码）"
    );
    assert!(!cut.contains('\u{fffd}'), "截断产生替换字符");
    assert_eq!(
        cut,
        format!("{BIG_CONTENT_A}{}", "中".repeat((cut.len() - 1) / 3)),
        "保留前缀应为 首字节A + N 个完整中文"
    );
}
