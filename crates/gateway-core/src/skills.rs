//! 技能（Skill）管理辅助：读取启用技能并注入转换请求的 system 上下文。
//! 也负责从 ZIP 解析技能包（bug 清单 #2）。

use std::io::{Cursor, Read};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::codec::ir::CanonicalRequest;
use crate::store::{self, SkillRow, StoreError};

/// ZIP 导入产生的技能草稿（尚未入库）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDraft {
    pub name: String,
    pub description: String,
    pub content: String,
}

#[derive(Deserialize)]
struct SkillsManifest {
    skills: Vec<SkillDraft>,
}

fn is_skill_text_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt")
}

fn skill_meta_from_file(name: &str, content: &str) -> (String, String) {
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string();
    let first_line = content
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_string();
    (stem, first_line)
}

/// 解析技能 ZIP：
/// - 若包含 `skills.json`（`{"skills":[{name,description,content}]}`），优先按清单导入；
/// - 否则把包内所有 `*.md` / `*.markdown` / `*.txt` 当作一个技能，
///   文件名（去掉扩展名）作为名称，第一行作为描述。
pub fn parse_skills_zip(bytes: &[u8]) -> Result<Vec<SkillDraft>, String> {
    let cursor = Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("不是有效的 ZIP 文件: {e}"))?;

    // 1) 支持 skills.json 清单
    if let Ok(mut file) = archive.by_name("skills.json") {
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|e| format!("读取 skills.json 失败: {e}"))?;
        let manifest: SkillsManifest =
            serde_json::from_str(&text).map_err(|e| format!("skills.json 解析失败: {e}"))?;
        let drafts: Vec<SkillDraft> = manifest
            .skills
            .into_iter()
            .filter(|s| !s.name.trim().is_empty() && !s.content.trim().is_empty())
            .collect();
        if !drafts.is_empty() {
            return Ok(drafts);
        }
    }

    // 2) 退路：每个文本文件一个技能
    let mut drafts = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("读取 ZIP 条目失败: {e}"))?;
        if file.is_dir() || file.name().starts_with("__MACOSX/") {
            continue;
        }
        let name = file.name().to_string();
        if !is_skill_text_file(&name) {
            continue;
        }
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| format!("读取 {} 失败: {e}", name))?;
        let content = content.trim().to_string();
        if content.is_empty() {
            continue;
        }
        let (skill_name, description) = skill_meta_from_file(&name, &content);
        drafts.push(SkillDraft {
            name: skill_name,
            description,
            content,
        });
    }

    if drafts.is_empty() {
        Err("ZIP 中未找到可导入的技能（支持 skills.json 清单或 *.md/*.txt）".into())
    } else {
        Ok(drafts)
    }
}

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
    use std::io::Write;

    #[test]
    fn parse_skills_zip_reads_manifest() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            w.start_file("skills.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(
                r#"{"skills":[{"name":"review","description":"代码评审","content":"按提交做评审"}]}"#
                    .as_bytes(),
            )
            .unwrap();
            w.finish().unwrap();
        }
        let drafts = parse_skills_zip(&buf).unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].name, "review");
        assert_eq!(drafts[0].description, "代码评审");
    }

    #[test]
    fn parse_skills_zip_reads_md_files() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            w.start_file("my-skill.md", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all("# 我的技能\n第一行作为描述\n正文内容".as_bytes())
                .unwrap();
            w.finish().unwrap();
        }
        let drafts = parse_skills_zip(&buf).unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].name, "my-skill");
        assert_eq!(drafts[0].description, "我的技能");
        assert!(drafts[0].content.contains("正文内容"));
    }

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
