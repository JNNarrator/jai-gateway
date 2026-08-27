//! 技能（Skill）管理辅助：读取启用技能并注入转换请求的 system 上下文。

use rusqlite::Connection;

use crate::codec::ir::CanonicalRequest;
use crate::store::{self, SkillRow, StoreError};

/// 读取所有启用的技能。
pub fn enabled_skills(c: &Connection) -> Result<Vec<SkillRow>, StoreError> {
    Ok(store::skill_list(c)?
        .into_iter()
        .filter(|s| s.enabled)
        .collect())
}

/// 把技能内容格式化为可追加到 system 的纯文本。
pub fn format_skills(skills: &[SkillRow]) -> String {
    let mut out = String::new();
    for (i, s) in skills.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format!("## 技能：{}", s.name));
        if !s.description.is_empty() {
            out.push_str(&format!("\n描述：{}", s.description));
        }
        out.push('\n');
        out.push_str(&s.content);
    }
    out
}

/// 将启用技能追加到 CanonicalRequest 的 system 中（仅转换路径调用）。
pub fn apply_enabled_skills(c: &Connection, req: &mut CanonicalRequest) {
    let Ok(skills) = enabled_skills(c) else {
        return;
    };
    if skills.is_empty() {
        return;
    }
    let text = format_skills(&skills);
    if req.system.is_empty() {
        req.system.push(text);
    } else {
        req.system[0] = format!("{}\n\n{}", req.system[0], text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_and_migrate;

    #[test]
    fn apply_enabled_skills_appends_system() {
        let c = open_and_migrate(":memory:").unwrap();
        let now = store::now_ms();
        let skill = SkillRow {
            id: "s1".into(),
            name: "code-review".into(),
            description: "审阅代码".into(),
            content: "请按提交变更做代码评审。".into(),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        store::skill_insert(&c, &skill).unwrap();
        let mut req = CanonicalRequest {
            model: "m".into(),
            system: vec!["You are helpful.".into()],
            messages: vec![],
            tools: vec![],
            tool_choice: Default::default(),
            params: Default::default(),
            stream: false,
            extensions: Default::default(),
        };
        apply_enabled_skills(&c, &mut req);
        assert!(req.system[0].contains("code-review"));
        assert!(req.system[0].contains("You are helpful."));
    }
}
