//! 网关密钥的白/黑名单（D9-T6b，迁移 0013）。
//!
//! 规则的意义是「把权限写进密钥本身」，而不是让客户端自觉：多密钥（D9-T6a）
//! 之后「一把密钥发给一个客户端」是常态，但默认每把密钥能碰到的渠道 / 模型
//! 完全一样 —— 发给 CI 流水线的那把同样能调最贵的模型。
//!
//! 语义（照 WaLiAPI 的收敛口径，简单可解释）：
//! - `deny` 命中 → 不可用（**优先于 allow**）
//! - `allow` 非空 → 只允许列表内的
//! - 两者都为空 → 不限制（向后兼容：没配规则的密钥行为与改造前完全一致）
//!
//! 两条轴相互独立、同时生效：渠道规则按 `provider_id`（渠道有稳定 UUID），
//! 模型规则按 `model_name`（同名模型常挂在多个渠道上，规则要能一次覆盖全渠道）。
//! 一个候选必须两条轴都通过才可用。

use rusqlite::Connection;
use std::collections::BTreeSet;

use super::StoreError;

/// 一把密钥的规则集合。全空 = 不限制。
///
/// 用 `BTreeSet` 而非 `Vec`：去重 + 稳定顺序（UI 列表与「一句话摘要」都要确定性输出，
/// 否则每次读出来顺序不同会让界面「跳」）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyRules {
    pub provider_allow: BTreeSet<String>,
    pub provider_deny: BTreeSet<String>,
    pub model_allow: BTreeSet<String>,
    pub model_deny: BTreeSet<String>,
}

impl KeyRules {
    /// 一条规则都没配 ⇒ 不限制。调用方可以据此整段跳过过滤逻辑（热路径快路）。
    pub fn is_empty(&self) -> bool {
        self.provider_allow.is_empty()
            && self.provider_deny.is_empty()
            && self.model_allow.is_empty()
            && self.model_deny.is_empty()
    }

    /// 渠道是否可用。
    pub fn allows_provider(&self, provider_id: &str) -> bool {
        axis_allows(&self.provider_allow, &self.provider_deny, provider_id)
    }

    /// 模型是否可用（按 `model_name`，不看渠道）。
    pub fn allows_model(&self, model_name: &str) -> bool {
        axis_allows(&self.model_allow, &self.model_deny, model_name)
    }
}

/// 单条轴的判定 —— 两条轴共用同一份语义，避免「渠道与模型规则各写一遍」之后
/// 某一边被改歪（这是很容易发生的分叉）。
fn axis_allows(allow: &BTreeSet<String>, deny: &BTreeSet<String>, item: &str) -> bool {
    if deny.contains(item) {
        return false;
    }
    if !allow.is_empty() {
        return allow.contains(item);
    }
    true
}

/// 读某把密钥的规则（没配过 → 全空）。
pub fn key_rules_get(c: &Connection, key_id: &str) -> Result<KeyRules, StoreError> {
    let mut out = KeyRules::default();
    read_axis(
        c,
        "key_provider_rules",
        "provider_id",
        key_id,
        &mut out.provider_allow,
        &mut out.provider_deny,
    )?;
    read_axis(
        c,
        "key_model_rules",
        "model_name",
        key_id,
        &mut out.model_allow,
        &mut out.model_deny,
    )?;
    Ok(out)
}

/// 覆盖式写入（先清空该密钥的全部规则行，再按 `r` 重写）。
///
/// 刻意做成「整表覆盖」而不是增删接口：UI 上规则是一整个表单一次性提交的，
/// 增量接口会引入「两边状态不一致」的中间态，而这张表极小，覆盖写的代价可忽略。
///
/// **同一项同时出现在 allow 与 deny 时按 deny 落地**（写库前从 allow 侧摘掉）：
/// 两张表的主键都是 `(key_id, item)`，不摘就会直接撞 UNIQUE 约束 —— 那样
/// 「deny 优先于 allow」对用户就变成了「保存失败」，毫无意义。于是读回来的规则里
/// 不可能出现「既允许又拒绝」，这正是 UI 三态选择器要保证的形态。
pub fn key_rules_set(c: &Connection, key_id: &str, r: &KeyRules) -> Result<(), StoreError> {
    let provider_allow: BTreeSet<String> = r
        .provider_allow
        .difference(&r.provider_deny)
        .cloned()
        .collect();
    let model_allow: BTreeSet<String> = r.model_allow.difference(&r.model_deny).cloned().collect();
    // 必须原子：清空与重写之间若失败，会留下「规则被清空但没写回」的**宽松窗口**
    // —— 权限被静默放大，这比拒绝更糟。
    let tx = c.unchecked_transaction()?;
    tx.execute("DELETE FROM key_provider_rules WHERE key_id=?1", [key_id])?;
    tx.execute("DELETE FROM key_model_rules WHERE key_id=?1", [key_id])?;
    write_axis(
        &tx,
        "key_provider_rules",
        "provider_id",
        key_id,
        &provider_allow,
        "allow",
    )?;
    write_axis(
        &tx,
        "key_provider_rules",
        "provider_id",
        key_id,
        &r.provider_deny,
        "deny",
    )?;
    write_axis(
        &tx,
        "key_model_rules",
        "model_name",
        key_id,
        &model_allow,
        "allow",
    )?;
    write_axis(
        &tx,
        "key_model_rules",
        "model_name",
        key_id,
        &r.model_deny,
        "deny",
    )?;
    tx.commit()?;
    Ok(())
}

// 表名 / 列名走 `format!` 拼进 SQL：这两个参数在调用点是**字面量常量**（上面四处
// 调用），不来自任何外部输入，所以没有注入面；参数化的只有 key_id / 规则项。
fn read_axis(
    c: &Connection,
    table: &str,
    col: &str,
    key_id: &str,
    allow: &mut BTreeSet<String>,
    deny: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let sql = format!("SELECT {col}, mode FROM {table} WHERE key_id=?1");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt
        .query_map([key_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (item, mode) in rows {
        if mode == "allow" {
            allow.insert(item);
        } else {
            // 迁移 0013 的 CHECK 约束保证只有 allow/deny；万一有人手改 DB 塞进别的值，
            // 按**更保守**的一侧（拒绝）处理，而不是当作放行。
            deny.insert(item);
        }
    }
    Ok(())
}

fn write_axis(
    c: &Connection,
    table: &str,
    col: &str,
    key_id: &str,
    items: &BTreeSet<String>,
    mode: &str,
) -> Result<(), StoreError> {
    if items.is_empty() {
        return Ok(());
    }
    let sql = format!("INSERT INTO {table}(key_id, {col}, mode) VALUES (?1, ?2, ?3)");
    let mut stmt = c.prepare(&sql)?;
    for item in items {
        stmt.execute(rusqlite::params![key_id, item, mode])?;
    }
    Ok(())
}

/// 规则选择器的一行候选：「某渠道上有某模型」。
///
/// 只列**启用**的渠道与模型 —— 规则 UI 要回答的是「这把密钥能用到什么」，
/// 而能被用到的首先是「网关本来就能服务的」；被停用的渠道等重新启用后自然会
/// 出现在这里（规则行本身不会因此失效）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOption {
    pub provider_id: String,
    pub provider_name: String,
    pub model_name: String,
}

/// 规则 UI 的候选清单（渠道 × 模型）。
///
/// 渠道列表由前端从这些行里去重得到 —— 多一条查询不如多一次 `Set` 去重，
/// 而且这样两边的顺序天然一致（按渠道优先级）。
pub fn rule_options(c: &Connection) -> Result<Vec<RuleOption>, StoreError> {
    let mut stmt = c.prepare(
        "SELECT p.id, p.name, m.model_name FROM models m \
         JOIN providers p ON p.id = m.provider_id \
         WHERE m.enabled = 1 AND p.enabled = 1 \
         ORDER BY p.priority ASC, p.name ASC, m.rowid ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RuleOption {
                provider_id: r.get(0)?,
                provider_name: r.get(1)?,
                model_name: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_and_migrate;

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    /// 语义矩阵：deny 命中 / allow 非空且命中 / allow 非空且未命中 / 都为空。
    #[test]
    fn axis_semantics_matrix() {
        let empty = KeyRules::default();
        assert!(empty.is_empty());
        assert!(empty.allows_provider("p1"), "都为空 ⇒ 不限制");
        assert!(empty.allows_model("m1"));

        let deny_only = KeyRules {
            provider_deny: set(&["p1"]),
            ..Default::default()
        };
        assert!(!deny_only.allows_provider("p1"), "deny 命中 ⇒ 不可用");
        assert!(deny_only.allows_provider("p2"), "未被 deny ⇒ 可用");
        assert!(
            deny_only.allows_model("m1"),
            "模型轴没配 ⇒ 不受渠道规则影响"
        );

        let allow_only = KeyRules {
            provider_allow: set(&["p1"]),
            ..Default::default()
        };
        assert!(allow_only.allows_provider("p1"), "allow 非空且命中");
        assert!(!allow_only.allows_provider("p2"), "allow 非空 ⇒ 白名单");

        // deny 优先于 allow：同一条目同时在两个集合里也必须拒绝。
        let both = KeyRules {
            provider_allow: set(&["p1", "p2"]),
            provider_deny: set(&["p1"]),
            ..Default::default()
        };
        assert!(!both.allows_provider("p1"), "deny 优先于 allow");
        assert!(both.allows_provider("p2"));

        // 两条轴独立：渠道被拒不影响模型轴自身的判定结果。
        let mixed = KeyRules {
            provider_deny: set(&["p1"]),
            model_allow: set(&["m1"]),
            ..Default::default()
        };
        assert!(!mixed.allows_provider("p1"));
        assert!(mixed.allows_model("m1"));
        assert!(!mixed.allows_model("m2"));
    }

    /// 规则行对 `providers(id)` 有外键（迁移 0013 的 ON DELETE CASCADE），所以
    /// 测试里引用的渠道必须先存在 —— 这正是真实调用路径的形态（规则项都来自
    /// 库里的渠道清单）。
    fn seed_provider(c: &Connection, id: &str) {
        crate::store::provider_insert(
            c,
            &crate::store::ProviderRow {
                id: id.into(),
                name: id.to_uppercase(),
                base_url: "http://127.0.0.1:1/v1".into(),
                family: "openai_compat".into(),
                enabled: true,
                priority: 100,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-x".into()),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                max_tools: None,
                reasoning_effort_levels: None,
                created_at: 1,
                updated_at: 1,
            },
        )
        .unwrap();
    }

    #[test]
    fn roundtrip_and_replace() {
        let c = open_and_migrate(":memory:").unwrap();
        for p in ["p1", "p2", "p9"] {
            seed_provider(&c, p);
        }
        assert_eq!(key_rules_get(&c, "k1").unwrap(), KeyRules::default());

        let mut r = KeyRules {
            provider_allow: set(&["p1"]),
            provider_deny: set(&["p2"]),
            model_deny: set(&["m-bad"]),
            ..Default::default()
        };
        key_rules_set(&c, "k1", &r).unwrap();
        assert_eq!(key_rules_get(&c, "k1").unwrap(), r);

        // 另一把密钥互不干扰
        assert_eq!(key_rules_get(&c, "k2").unwrap(), KeyRules::default());

        // 覆盖式：新集合里没有的项必须消失（不是增量追加）
        r.provider_deny = set(&["p9"]);
        r.model_deny = BTreeSet::new();
        key_rules_set(&c, "k1", &r).unwrap();
        let got = key_rules_get(&c, "k1").unwrap();
        assert_eq!(got, r);
        assert!(!got.provider_deny.contains("p2"), "覆盖写必须清掉旧项");

        // 清空 ⇒ 回到「不限制」
        key_rules_set(&c, "k1", &KeyRules::default()).unwrap();
        assert!(key_rules_get(&c, "k1").unwrap().is_empty());
    }

    /// 同一项同时出现在 allow 与 deny ⇒ 保存时按 deny 落地（**不报错**）。
    ///
    /// 两张表的主键都是 `(key_id, item)`，若照原样写就会撞 UNIQUE 约束。把冲突
    /// 留给用户去「保存失败」是没有意义的：语义上 deny 本来就优先。
    #[test]
    fn conflicting_allow_and_deny_resolves_to_deny() {
        let c = open_and_migrate(":memory:").unwrap();
        for p in ["p1", "p2"] {
            seed_provider(&c, p);
        }
        let r = KeyRules {
            provider_allow: set(&["p1", "p2"]),
            provider_deny: set(&["p1"]),
            model_allow: set(&["m1"]),
            model_deny: set(&["m1"]),
        };
        key_rules_set(&c, "k1", &r).unwrap();

        let got = key_rules_get(&c, "k1").unwrap();
        assert!(got.provider_deny.contains("p1"));
        assert!(
            !got.provider_allow.contains("p1"),
            "冲突项必须从 allow 侧摘掉"
        );
        assert!(got.provider_allow.contains("p2"));
        assert!(got.model_deny.contains("m1"));
        assert!(got.model_allow.is_empty());
        assert!(!got.allows_provider("p1"), "落库后判定与 deny 优先一致");
        assert!(got.allows_provider("p2"));
    }

    /// 删渠道必须把它的规则行一起带走（迁移里的 `ON DELETE CASCADE`）。
    ///
    /// 没有 CASCADE 时 `DELETE FROM providers` 会直接撞外键冲突 —— 那会让
    /// 「删除渠道」这个再普通不过的操作变成一个报错，所以这条要守着。
    #[test]
    fn deleting_provider_cascades_rule_rows() {
        let c = open_and_migrate(":memory:").unwrap();
        seed_provider(&c, "p1");
        let r = KeyRules {
            provider_deny: set(&["p1"]),
            ..Default::default()
        };
        key_rules_set(&c, "k1", &r).unwrap();
        crate::store::provider_delete(&c, "p1").unwrap();
        assert!(
            key_rules_get(&c, "k1").unwrap().provider_deny.is_empty(),
            "渠道删除后规则行必须一并消失"
        );
    }
}
