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
