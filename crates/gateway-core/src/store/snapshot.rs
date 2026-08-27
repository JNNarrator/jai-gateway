//! 模型元数据快照 —— 需求 §2：热门模型默认值（上下文窗口/最大输出）。
//! 来源参考 LiteLLM registry，随版本发布更新（待决策事项 #4：是否联网刷新）。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotEntry {
    pub name: String,
    pub ctx: i64,
    pub out: i64,
}

/// (context_window, max_output_tokens)
pub fn lookup(model_name: &str) -> Option<(i64, i64)> {
    static SNAPSHOT: std::sync::OnceLock<Vec<SnapshotEntry>> = std::sync::OnceLock::new();
    let list = SNAPSHOT.get_or_init(|| {
        serde_json::from_str::<Vec<SnapshotEntry>>(include_str!("snapshot.json"))
            .expect("内嵌 snapshot.json 必须合法")
    });
    list.iter()
        .find(|e| e.name == model_name)
        .map(|e| (e.ctx, e.out))
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_and_unknown_models() {
        assert_eq!(super::lookup("gpt-4o"), Some((128000, 16384)));
        assert_eq!(super::lookup("deepseek-chat"), Some((65536, 8192)));
        assert_eq!(super::lookup("totally-made-up-model"), None);
    }

    #[test]
    fn snapshot_file_is_sorted_unique() {
        let raw = include_str!("snapshot.json");
        let list: Vec<super::SnapshotEntry> = serde_json::from_str(raw).unwrap();
        let mut names: Vec<_> = list.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "快照中存在重复模型名");
    }
}
