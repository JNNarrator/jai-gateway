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
