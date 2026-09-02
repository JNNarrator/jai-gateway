//! MCP / Skill 管理功能测试：迁移建表 + CRUD + 启停。

use gateway_core::store::{self, Db};

#[test]
fn mcp_and_skill_crud() {
    let db = Db::in_memory().unwrap();
    let now = store::now_ms();

    db.with(|c| {
        let mcp = store::McpServerRow {
            id: "m1".into(),
            name: "filesystem".into(),
            kind: "stdio".into(),
            command: Some("npx".into()),
            args: Some("[\"-y\",\"@modelcontextprotocol/server-filesystem\"]".into()),
            url: None,
            env: None,
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        store::mcp_insert(c, &mcp)?;
        assert_eq!(store::mcp_list(c)?.len(), 1);

        store::mcp_update(
            c,
            "m1",
            "filesystem",
            "sse",
            None,
            None,
            Some("https://mcp.local/sse"),
            None,
        )?;
        store::mcp_set_enabled(c, "m1", false)?;
        let list = store::mcp_list(c)?;
        assert!(!list[0].enabled);
        assert_eq!(list[0].kind, "sse");
        assert_eq!(store::mcp_delete(c, "m1")?, 1);

        let skill = store::SkillRow {
            id: "s1".into(),
            name: "code-review".into(),
            description: "run review".into(),
            content: "请按提交变更做代码评审…".into(),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        store::skill_insert(c, &skill)?;
        assert_eq!(store::skill_list(c)?.len(), 1);
        store::skill_update(c, "s1", "code-review", "desc", "new content")?;
        store::skill_set_enabled(c, "s1", false)?;
        let list = store::skill_list(c)?;
        assert!(!list[0].enabled);
        assert_eq!(list[0].content, "new content");
        assert_eq!(store::skill_delete(c, "s1")?, 1);

        Ok::<_, store::StoreError>(())
    })
    .unwrap();
}

#[test]
fn parse_mcp_servers_json_supports_claude_code_format() {
    // 用户实际粘贴的 Claude Code 格式：env + args + command
    let json_text = r#"{
        "mcpServers": {
            "netcatty-external": {
                "command": "/Applications/Netcatty.app/Contents/Resources/app.asar.unpacked/electron/cli/netcatty-external-mcp",
                "args": [],
                "env": {
                    "NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE": "/Users/jiangnan/Library/Application Support/netcatty/external-mcp/discovery.json"
                }
            },
            "remote": {"type": "http", "url": "https://mcp.example.com/mcp"},
            "bad-type": {"type": "kafka", "command": "x"},
            "missing-cmd": {"env": {}}
        }
    }"#;
    let entries = store::parse_mcp_servers_json(json_text).unwrap();
    assert_eq!(entries.len(), 4);

    // 1) stdio + env：用户样例完整解析
    let nc = entries
        .iter()
        .find(|e| e.name == "netcatty-external")
        .unwrap();
    assert_eq!(nc.kind, "stdio");
    assert!(nc
        .command
        .as_deref()
        .unwrap()
        .contains("netcatty-external-mcp"));
    assert_eq!(nc.args.as_deref(), Some("[]"));
    assert!(nc
        .env
        .as_deref()
        .unwrap()
        .contains("NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE"));
    assert!(nc.skip_reason.is_none());

    // 2) http + type + url
    let remote = entries.iter().find(|e| e.name == "remote").unwrap();
    assert_eq!(remote.kind, "http");
    assert_eq!(remote.url.as_deref(), Some("https://mcp.example.com/mcp"));
    assert!(remote.skip_reason.is_none());

    // 3) 非法 type 标记跳过
    let bad = entries.iter().find(|e| e.name == "bad-type").unwrap();
    assert!(bad.skip_reason.is_some());

    // 4) stdio 缺 command 标记跳过
    let missing = entries.iter().find(|e| e.name == "missing-cmd").unwrap();
    assert!(missing.skip_reason.is_some());

    // 顶层非 mcpServers / 空 / 坏 JSON 均报错
    assert!(store::parse_mcp_servers_json("{\"foo\":1}").is_err());
    assert!(store::parse_mcp_servers_json("{\"mcpServers\":{}}").is_err());
    assert!(store::parse_mcp_servers_json("not json").is_err());
}

#[test]
fn parse_mcp_servers_json_supports_bare_object() {
    // 无 mcpServers 包装的裸对象（值都含 command/url 时视作 server 映射）
    let entries = store::parse_mcp_servers_json(
        r#"{"nc": {"command": "a.cmd", "env": {"K": "V"}},
            "r": {"url": "https://mcp.example.com/mcp"}}"#,
    )
    .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "nc");
    assert_eq!(entries[0].env.as_deref(), Some(r#"{"K":"V"}"#));
    assert_eq!(entries[1].kind, "http");
    // 混入非对象值时不视作裸对象，仍报缺 mcpServers
    assert!(store::parse_mcp_servers_json(r#"{"nc": {"command": "a"}, "n": 1}"#).is_err());
}

#[test]
fn parse_mcp_servers_toml_supports_codex_config() {
    // Codex config.toml 片段（用户实际样例）
    let toml_text = r#"
[mcp_servers.netcatty-external]
command = "C:\\Program Files\\Netcatty\\resources\\app.asar.unpacked\\electron\\cli\\netcatty-external-mcp.cmd"
args = []
env = { NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE = "C:\\Users\\WM\\AppData\\Roaming\\netcatty\\external-mcp\\discovery.json" }
"#;
    let entries = store::parse_mcp_servers_toml(toml_text).unwrap();
    assert_eq!(entries.len(), 1);
    let nc = &entries[0];
    assert_eq!(nc.name, "netcatty-external");
    assert_eq!(nc.kind, "stdio");
    assert!(nc
        .command
        .as_deref()
        .unwrap()
        .contains("netcatty-external-mcp.cmd"));
    assert_eq!(nc.args.as_deref(), Some("[]"));
    assert!(nc
        .env
        .as_deref()
        .unwrap()
        .contains("NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE"));
    assert!(nc.skip_reason.is_none());

    // url 条目（Streamable HTTP）与 sse transport
    let entries = store::parse_mcp_servers_toml(
        r#"
[mcp_servers.docs]
url = "https://docs.mcp.example.com/mcp"
[mcp_servers.sse1]
url = "https://sse.mcp.example.com/sse"
transport = "sse"
"#,
    )
    .unwrap();
    assert_eq!(entries[0].kind, "http");
    assert!(entries[0].skip_reason.is_none());
    assert_eq!(entries[1].kind, "sse");

    // 缺表头 / 空 / 坏 TOML 报错
    assert!(store::parse_mcp_servers_toml("command = \"x\"").is_err());
    assert!(store::parse_mcp_servers_toml("[mcp_servers]").is_err());
    assert!(store::parse_mcp_servers_toml("[[[bad").is_err());
}

#[test]
fn parse_mcp_add_cli_supports_codex_command() {
    // Codex CLI 命令行（用户实际样例，含引号内空格路径）
    let cli = concat!(
        "codex mcp add netcatty-external ",
        "--env \"NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE=C:\\Users\\WM\\AppData\\Roaming\\netcatty\\external-mcp\\discovery.json\" ",
        "-- \"C:\\Program Files\\Netcatty\\resources\\app.asar.unpacked\\electron\\cli\\netcatty-external-mcp.cmd\""
    );
    let entries = store::parse_mcp_add_cli(cli).unwrap();
    assert_eq!(entries.len(), 1);
    let nc = &entries[0];
    assert_eq!(nc.name, "netcatty-external");
    assert_eq!(nc.kind, "stdio");
    assert_eq!(
        nc.command.as_deref().unwrap(),
        "C:\\Program Files\\Netcatty\\resources\\app.asar.unpacked\\electron\\cli\\netcatty-external-mcp.cmd"
    );
    assert!(nc.args.is_none());
    let env = nc.env.as_deref().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(env).unwrap();
    assert_eq!(
        parsed["NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE"],
        "C:\\Users\\WM\\AppData\\Roaming\\netcatty\\external-mcp\\discovery.json"
    );
    assert!(nc.skip_reason.is_none());

    // 无 `--`：名称后跟命令 + 多个参数
    let entries =
        store::parse_mcp_add_cli("codex mcp add fs npx -y @modelcontextprotocol/server-fs")
            .unwrap();
    let fs = &entries[0];
    assert_eq!(fs.command.as_deref(), Some("npx"));
    assert_eq!(
        fs.args.as_deref(),
        Some(r#"["-y","@modelcontextprotocol/server-fs"]"#)
    );

    // 多个 --env + claude 前缀 + http url
    let entries = store::parse_mcp_add_cli(
        "claude mcp add r --env A=1 --env B=2 --url https://mcp.example.com/mcp",
    )
    .unwrap();
    let r = &entries[0];
    assert_eq!(r.kind, "http");
    assert_eq!(r.url.as_deref(), Some("https://mcp.example.com/mcp"));
    let env: serde_json::Value = serde_json::from_str(r.env.as_deref().unwrap()).unwrap();
    assert_eq!(env["A"], "1");
    assert_eq!(env["B"], "2");

    // 缺 command（只有名称）→ 跳过；env 非法 KEY=VALUE → 跳过
    let entries = store::parse_mcp_add_cli("codex mcp add lonely").unwrap();
    assert!(entries[0].skip_reason.is_some());
    let entries = store::parse_mcp_add_cli("codex mcp add x --env BAD -- echo hi").unwrap();
    assert!(entries[0].skip_reason.is_some());
    // 名称缺失直接报错
    assert!(store::parse_mcp_add_cli("codex mcp add -- echo hi").is_err());
}

#[test]
fn parse_mcp_import_autodetects_three_formats() {
    // 1) CLI 命令行
    let cli = "codex mcp add netcatty-external -- \"C:\\Program Files\\Netcatty\\cli\\netcatty-external-mcp.cmd\"";
    let entries = store::parse_mcp_import(cli).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, "stdio");

    // 2) mcpServers JSON（原格式不回归）
    let entries = store::parse_mcp_import(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
    assert_eq!(entries[0].name, "a");

    // 3) Codex TOML
    let entries = store::parse_mcp_import(
        "[mcp_servers.b]\ncommand = \"C:\\\\path with space\\\\x.cmd\"\nenv = { K = \"V\" }",
    )
    .unwrap();
    assert_eq!(entries[0].name, "b");
    assert!(entries[0].env.as_deref().unwrap().contains("\"K\":\"V\""));

    // 空内容 / 无法识别
    assert!(store::parse_mcp_import("   ").is_err());
    assert!(store::parse_mcp_import("hello world").is_err());
    // shell 提示符前缀容忍
    let entries = store::parse_mcp_import("$ codex mcp add n -- echo hi").unwrap();
    assert_eq!(entries[0].command.as_deref(), Some("echo"));
}
