//! 内嵌迁移脚本。SQL 为唯一权威，Rust 不做第二份 schema 描述。
//! 新增迁移 = 在数组尾部追加一条，禁止修改历史条目。

/// (名称, SQL)
pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_initial_schema",
        include_str!("migrations/0001_initial_schema.sql"),
    ),
    (
        "0002_mcp_and_skills",
        include_str!("migrations/0002_mcp_and_skills.sql"),
    ),
    (
        "0003_openai_responses_family",
        include_str!("migrations/0003_openai_responses_family.sql"),
    ),
    (
        "0004_advanced_routing",
        include_str!("migrations/0004_advanced_routing.sql"),
    ),
    ("0005_mcp_env", include_str!("migrations/0005_mcp_env.sql")),
    (
        "0006_secrets_in_db",
        include_str!("migrations/0006_secrets_in_db.sql"),
    ),
    (
        "0007_mcp_proxy_allowed",
        include_str!("migrations/0007_mcp_proxy_allowed.sql"),
    ),
    (
        "0008_proxy_call_logs",
        include_str!("migrations/0008_proxy_call_logs.sql"),
    ),
    (
        "0009_model_support",
        include_str!("migrations/0009_model_support.sql"),
    ),
    (
        "0010_model_modalities",
        include_str!("migrations/0010_model_modalities.sql"),
    ),
    (
        "0011_reasoning_effort_levels",
        include_str!("migrations/0011_reasoning_effort_levels.sql"),
    ),
    (
        "0012_max_tools_per_channel",
        include_str!("migrations/0012_max_tools_per_channel.sql"),
    ),
    (
        "0013_key_rules",
        include_str!("migrations/0013_key_rules.sql"),
    ),
];

#[cfg(test)]
mod tests {
    #[test]
    fn migration_names_are_unique_and_ordered() {
        let mut prev = String::new();
        for (name, _) in super::MIGRATIONS {
            assert_ne!(name, &prev, "重复的迁移名: {name}");
            prev = (*name).to_string();
        }
    }
}
