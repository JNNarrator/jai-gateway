//! 推理回放兼容（reasoning replay）—— **自适应**的「该上游要求 assistant 历史带推理」适配。
//!
//! ## 问题
//!
//! DeepSeek 系 thinking 模型要求**每个回放的 assistant 消息都带推理字段**，否则 400。
//! 官方 `deepseek.com` 接受空串 `""`，但部分第三方中继**拒绝空串、要求非空值**
//! （PI-Desktop 的 `#296` 记录，其 `requiresNonEmptyReasoningReplay` 就是为这个）。
//!
//! 而客户端**本来就会**把历史里的推理裁掉：
//! - PI-Desktop 只保留最近 3 轮思考（`MAX_RETAINED_REASONING_TURNS = 3`）；
//! - 跨模型回放时主动删（`removeCrossModelReasoning`）；
//! - 上下文压缩后、纯工具调用轮、模型那轮确实没思考。
//!
//! 于是跨族转换（客户端说 Anthropic/Responses × 上游是 openai_compat）出站时，
//! 很多 assistant 轮次**没有推理字段** → 严格中继 400。
//! 同族直通不受影响（字节转发，客户端自己的历史原样到达）。
//!
//! ## 为什么不是「一个渠道开关」
//!
//! 上游可能很多、且随时新增，靠人配是配不完的，靠猜模型名也会漏（PI-Desktop 自己的
//! 注释就承认「聚合器和自定义网关匹配不上」）。所以这里做成**三层、逐层收紧**：
//!
//! 1. **推断**（[`infer_from_channel`]，零额外往返）：渠道的模型名 / base_url /
//!    供应商名带明显 DeepSeek 标记时，直接按「需要非空回放」处理。覆盖常见情形。
//! 2. **学习**（[`Registry`] + 上游真实报错）：没猜中的上游，第一次被它 400 后
//!    **识别报错 → 记下来 → 用兼容重试同一请求**，客户端只看到成功。
//!    **只有重试真的成功才持久化** —— 学习结果自我验证，误判不会留下来。
//! 3. **遗忘**：已开启的渠道若再次报同样的错，说明这个开关对它无效 → 清除标记，
//!    回到不干预（避免把一个本来能跑的配置永久改坏）。
//!
//! 持久化用现成的 `meta` KV 表（`key TEXT PRIMARY KEY, value TEXT`）—— **不需要迁移**。
//! 标记是**供应商级**的（兼容性是中转端点的属性，不是单个模型的）。

use crate::store::{self, Db};
use serde_json::Value;

/// 请求面内部键：规划层据此告诉 `openai` encoder「该渠道需要非空推理回放」。
/// 与 `__jai_validate_output` / `__jai_tool_identities` 同一套机制（`req.extensions`），
/// 这样**不必改四个 `encode_request` 的签名**。
pub const EXT_KEY: &str = "__jai_reasoning_replay";

/// 历史里确实没有推理时填的**非空**占位。
///
/// 只在上游明确要求非空（推断命中或学习命中）时才会出现；默认路径完全不发明内容。
/// 措辞刻意保持中性、可被模型忽略。PI-Desktop 用同义英文串
/// `"[reasoning not retained for this turn]"`；该字面量只是各自实现的约定，不是协议常量。
pub const PLACEHOLDER: &str = "[reasoning not retained for this turn]";

/// `meta` 键前缀（后接 provider id）。
const META_PREFIX: &str = "reasoning_replay:";

fn meta_key(provider_id: &str) -> String {
    format!("{META_PREFIX}{provider_id}")
}

// ================================================================ 第 1 层：推断

/// 渠道是否**明显**属于 DeepSeek 家族（模型名 / base_url / 供应商名带标记）。
///
/// 保守：只在真出现 `deepseek` 字样时才命中，不做「thinking 模型都算」这类外推
/// （那会把大量非 DeepSeek 上游卷进来）。猜错的兜底是第 2/3 层。
pub fn infer_from_channel(model: &str, base_url: &str, provider_name: &str) -> bool {
    [model, base_url, provider_name]
        .iter()
        .any(|s| s.to_ascii_lowercase().contains("deepseek"))
}

// ================================================================ 第 2 层：报错识别

/// 上游报错是否属于「要求回放推理」这一类。
///
/// 必须**窄**：误判的代价是给一个不需要该字段的上游塞占位（可能把能跑的配置改坏），
/// 所以要求「提到推理字段」**且**「要求存在/非空」两个条件同时成立，并且只认 4xx。
/// 各家措辞不一致，这里只匹配语义明确的一组。
pub fn is_replay_rejection(status: u16, body_excerpt: &str) -> bool {
    if !(400..500).contains(&status) {
        return false;
    }
    let s = body_excerpt.to_ascii_lowercase();
    // ① 必须点名推理字段（三种线上拼写之一）
    let names_field = s.contains("reasoning_content")
        || s.contains("reasoning_text")
        || s.contains("reasoning field");
    if !names_field {
        return false;
    }
    // ② 且必须是「要求存在 / 不能为空」语义，而不是值域、格式之类的其它抱怨
    const REQUIREMENT_MARKERS: [&str; 6] = [
        "required",
        "must be",
        "must not be empty",
        "cannot be empty",
        "not provided",
        "missing",
    ];
    REQUIREMENT_MARKERS.iter().any(|m| s.contains(m))
}

// ================================================================ 第 2/3 层：学习与遗忘

/// 「该渠道需要非空推理回放」的标记表：`meta` 之上的**写穿缓存**。
///
/// 三态：`None` = 未学习（回落到[推断][infer_from_channel]）；
/// `Some(true)` = 学习到「需要」；`Some(false)` = 学习到「不需要 / 无效」。
/// 有了 `Some(false)` 才能**抑制错误的推断** —— 否则「模型名像 DeepSeek 但上游其实
/// 不需要占位」的渠道会被永久塞占位，把本来能跑的配置改坏。
///
/// 读路径几乎不碰 DB（缓存未命中才查一次），写路径同步落库，进程重启后仍然记得。
#[derive(Default)]
pub struct Registry {
    cache: std::sync::Mutex<std::collections::HashMap<String, Option<bool>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 学习到的值；`None` = 未学习（调用方应回落到推断）。未命中缓存则查 `meta`。
    pub fn get(&self, db: &Db, provider_id: &str) -> Option<bool> {
        if let Ok(c) = self.cache.lock() {
            if let Some(v) = c.get(provider_id) {
                return *v;
            }
        }
        // meta 里存 "true" / "false"；键不存在 → None（未学习）
        let loaded = db
            .with_any(|c| store::meta_get(c, &meta_key(provider_id)))
            .ok()
            .flatten()
            .and_then(|v| match v.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });
        if let Ok(mut c) = self.cache.lock() {
            c.insert(provider_id.to_string(), loaded);
        }
        loaded
    }

    /// 记下「需要非空回放」并落库。
    pub fn learn(&self, db: &Db, provider_id: &str) {
        self.set(db, provider_id, Some(true));
    }

    /// 记下「不需要 / 该开关无效」并落库 —— 用于抑制错误的推断，避免反复塞占位。
    pub fn suppress(&self, db: &Db, provider_id: &str) {
        self.set(db, provider_id, Some(false));
    }

    fn set(&self, db: &Db, provider_id: &str, value: Option<bool>) {
        if let Ok(mut c) = self.cache.lock() {
            c.insert(provider_id.to_string(), value);
        }
        // 三个分支的 Result 载荷类型不同（() / usize），统一用 let _ 吞掉错误即可
        match value {
            Some(true) => {
                let _ = db.with_any(|c| store::meta_set(c, &meta_key(provider_id), "true"));
            }
            Some(false) => {
                let _ = db.with_any(|c| store::meta_set(c, &meta_key(provider_id), "false"));
            }
            None => {
                let _ = db.with_any(|c| store::meta_delete(c, &meta_key(provider_id)));
            }
        }
    }
}

/// 该渠道本次请求是否需要「非空推理回放」。
///
/// **学习值优先于推断**（三态语义）：学到 `true` 就用，学到 `false` 就不用（哪怕模型名
/// 像 DeepSeek），未学习才回落到推断。
pub fn enabled_for(registry: &Registry, db: &Db, cand: &store::RouteCandidate) -> bool {
    match registry.get(db, &cand.provider_id) {
        Some(learned) => learned,
        None => infer_from_channel(
            cand.upstream_model_id.as_deref().unwrap_or_default(),
            &cand.base_url,
            &cand.provider_name,
        ),
    }
}

// ================================================ 第 2 层（直通路径）：JSON 占位注入

/// 同族直通路径的兜底：给**缺推理字段**的 assistant 消息补上非空占位。
///
/// 直通路径是字节转发（不经 IR、不过 `openai` 编码器），所以只能在原始 JSON 上做手术。
/// 只在 [`enabled_for`] 为真时才被调用（推断命中或学习命中）；默认路径一个字节都不改。
/// 解析失败 / 没有 `messages` / 一条都没补 → `None`，调用方保持原字节
/// （宁可不干预，也不弄坏一个本来能跑的请求）。
///
/// 补进去的名字固定用 `reasoning_content`（PI-Desktop 清单里的首选拼写）；只有当消息上
/// **三个已知拼写都没有**时才补，所以客户端自己带过推理的轮次不会被覆盖。
pub fn inject_placeholders(raw: &[u8]) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(raw).ok()?;
    let msgs = v.get_mut("messages")?.as_array_mut()?;
    let mut touched = false;
    for m in msgs.iter_mut() {
        let Some(obj) = m.as_object_mut() else {
            continue;
        };
        if obj.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let has = crate::codec::openai::REASONING_FIELDS
            .iter()
            .any(|f| obj.contains_key(*f));
        if !has {
            obj.insert(
                crate::codec::openai::REASONING_FIELDS[0].to_string(),
                Value::String(PLACEHOLDER.to_string()),
            );
            touched = true;
        }
    }
    if !touched {
        return None;
    }
    serde_json::to_vec(&v).ok()
}

/// 自适应闭环的入口：识别到「要求回放推理」的 400 后，决定**学习并重试**还是**不再干预**。
///
/// 返回 `true` = 该渠道**从未试过**这个开关 ⇒ 调用方应学习后**原地重试一次**
/// （客户端只看到成功）。返回 `false` = 不再重试。
///
/// **不抖动**是这里的关键：只在「未学习（`None`）」时做一次判断，一旦落到 `Some(_)`
/// 就不再改变 —— 否则「学习 → 失败 → 抑制 → 再学习」会每请求来回翻，反而制造噪声。
///
/// 两种「不重试」的成因不同：
/// - 未学习但**推断已让它生效**：说明推断不够用（上游不是缺字段的问题）⇒ 记 `Some(false)`
///   压住推断，避免继续给这个渠道塞占位；
/// - 已学习（`Some(true)`）却仍被拒：调用方在**重试也失败**后调 [`Registry::suppress`]
///   （见 proxy 的重试分支）—— 只有重试成功才保留学习结果，这是自我验证。
pub fn should_retry_after_rejection(
    registry: &Registry,
    db: &Db,
    cand: &store::RouteCandidate,
) -> bool {
    match registry.get(db, &cand.provider_id) {
        // 已经学到过结论（true 或 false）→ 不再改状态、不再重试
        Some(_) => false,
        None => {
            if enabled_for(registry, db, cand) {
                // 推断已生效却仍被拒 → 推断不够用，抑制掉
                registry.suppress(db, &cand.provider_id);
                false
            } else {
                registry.learn(db, &cand.provider_id);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的最小候选（`RouteCandidate` 没有 `Default`，逐字段给全）。
    fn cand_of(provider_name: &str, base_url: &str, model: &str) -> store::RouteCandidate {
        store::RouteCandidate {
            provider_id: "p-test".into(),
            provider_name: provider_name.into(),
            priority: 1,
            base_url: base_url.into(),
            family: "openai_compat".into(),
            extra_headers: None,
            api_key: None,
            website: None,
            upstream_model_id: Some(model.into()),
            max_output_tokens: 4096,
            weight: 1,
            last_ok_at: None,
            last_err_at: None,
            reasoning_effort_levels: None,
            max_tools: None,
        }
    }

    fn tmp_db(tag: &str) -> Db {
        let dir = std::env::temp_dir().join(format!(
            "jai-replay-{tag}-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Db::open(dir.join("main.db").to_str().unwrap()).unwrap()
    }

    #[test]
    fn infer_only_matches_clear_deepseek_markers() {
        assert!(infer_from_channel("deepseek-v4-flash", "", ""));
        assert!(infer_from_channel(
            "",
            "https://relay.example.com/deepseek",
            ""
        ));
        assert!(infer_from_channel("", "", "My DeepSeek Relay"));
        assert!(infer_from_channel("DeepSeek-V4", "", ""));
        // 不臆断：只是「thinking 模型」不算
        assert!(!infer_from_channel(
            "glm-5",
            "https://api.z.ai/api/paas/v4",
            "基元律动"
        ));
        assert!(!infer_from_channel(
            "qwen3.5-plus",
            "https://dashscope.aliyuncs.com",
            ""
        ));
    }

    #[test]
    fn rejection_matcher_is_narrow() {
        // 命中：点名字段 + 要求语义
        assert!(is_replay_rejection(
            400,
            r#"{"error":{"message":"reasoning_content is required for assistant messages"}}"#
        ));
        assert!(is_replay_rejection(
            400,
            r#"{"message":"reasoning_content cannot be empty"}"#
        ));
        assert!(is_replay_rejection(
            422,
            r#"{"detail":"reasoning_text not provided"}"#
        ));
        // 不命中：值域抱怨（那属 0011 推理档位值域的事，不该被当成回放要求）
        assert!(!is_replay_rejection(
            400,
            r#"{"message":"DeepSeek reasoning_effort 只支持 low、medium、high"}"#
        ));
        // 不命中：没点名推理字段
        assert!(!is_replay_rejection(
            400,
            r#"{"message":"model is required"}"#
        ));
        // 不命中：非 4xx
        assert!(!is_replay_rejection(
            500,
            r#"{"message":"reasoning_content required"}"#
        ));
    }

    #[test]
    fn registry_states_persist_across_restart() {
        let db = tmp_db("persist");
        let reg = Registry::new();

        assert_eq!(reg.get(&db, "p1"), None, "默认未学习（回落到推断）");
        reg.learn(&db, "p1");
        assert_eq!(reg.get(&db, "p1"), Some(true));
        assert_eq!(
            Registry::new().get(&db, "p1"),
            Some(true),
            "学习结果应跨进程存活"
        );

        // suppress 是「学到不需要」，与「未学习」不同：它要压住推断
        reg.suppress(&db, "p2");
        assert_eq!(
            Registry::new().get(&db, "p2"),
            Some(false),
            "抑制状态也应跨进程存活"
        );
    }

    /// 三态语义：学习值压过推断；未学习才回落到推断。
    #[test]
    fn learned_value_overrides_inference() {
        let db = tmp_db("tristate");
        let reg = Registry::new();
        // 模型名像 DeepSeek → 未学习时推断命中
        let cand = cand_of(
            "某中转",
            "https://relay.example.com/v1",
            "deepseek-v4-flash",
        );
        assert!(enabled_for(&reg, &db, &cand), "推断命中");

        // 学到「不需要」→ 压住推断（否则这个渠道会被永久塞占位）
        reg.suppress(&db, &cand.provider_id);
        assert!(!enabled_for(&reg, &db, &cand), "抑制应压过推断");
    }

    /// 自适应闭环（不抖动）：
    /// 首次「学习 + 重试」→ 重试仍失败则抑制 → 之后不再学、不再重试。
    #[test]
    fn learn_then_suppress_without_oscillation() {
        let db = tmp_db("learn");
        let reg = Registry::new();
        let cand = cand_of("未知中转", "https://relay.example.com/v1", "some-model");

        // 第一次：从未试过 → 学习 + 要求重试
        assert!(should_retry_after_rejection(&reg, &db, &cand));
        assert_eq!(reg.get(&db, &cand.provider_id), Some(true));

        // 重试也失败 → 调用方抑制（自我验证：只有重试成功才保留学习结果）
        reg.suppress(&db, &cand.provider_id);
        assert!(!enabled_for(&reg, &db, &cand));

        // 之后每次都「不重试、不改状态」——不会学习/抑制来回翻
        for _ in 0..3 {
            assert!(!should_retry_after_rejection(&reg, &db, &cand));
            assert_eq!(reg.get(&db, &cand.provider_id), Some(false));
        }
    }

    /// 推断已生效却仍被拒 → 说明推断不够用，抑制掉且不重试（同样不抖动）。
    #[test]
    fn inferred_but_still_rejected_gets_suppressed() {
        let db = tmp_db("inferred");
        let reg = Registry::new();
        let cand = cand_of(
            "某中转",
            "https://relay.example.com/v1",
            "deepseek-v4-flash",
        );

        // 推断命中 → 开关已生效
        assert!(enabled_for(&reg, &db, &cand));
        // 仍被拒 → 不重试，且记下「不需要」压住推断
        assert!(!should_retry_after_rejection(&reg, &db, &cand));
        assert_eq!(reg.get(&db, &cand.provider_id), Some(false));
        assert!(!enabled_for(&reg, &db, &cand));
        // 后续保持稳定
        assert!(!should_retry_after_rejection(&reg, &db, &cand));
    }
    /// 直通路径注入：只补缺字段的 assistant 消息，其余一字不改；
    /// 一条都不缺 → `None`（调用方保持原字节）。
    #[test]
    fn inject_placeholders_only_touches_assistant_without_reasoning() {
        // 有 assistant 缺推理 → 补上（user 不动、已有推理的不动）
        let raw = br#"{"model":"m","messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","content":"a1"},
            {"role":"assistant","content":"a2","reasoning_content":"real thinking"},
            {"role":"tool","content":"t"}
        ]}"#;
        let out = inject_placeholders(raw).expect("应补上占位");
        let v: Value = serde_json::from_slice(&out).unwrap();
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs[0].get("reasoning_content"), None, "user 不动");
        assert_eq!(
            msgs[1]["reasoning_content"].as_str(),
            Some(PLACEHOLDER),
            "缺推理的 assistant 应补非空占位"
        );
        assert_eq!(
            msgs[2]["reasoning_content"].as_str(),
            Some("real thinking"),
            "客户端自带的推理不能被覆盖"
        );
        assert_eq!(msgs[3].get("reasoning_content"), None, "tool 不动");

        // 三个已知拼写之一存在就算「不缺」，不重复补
        let raw2 = br#"{"messages":[{"role":"assistant","content":"a","reasoning":"r"}]}"#;
        assert!(inject_placeholders(raw2).is_none());

        // 没有 assistant / 没有 messages / 不是 JSON → 不干预
        assert!(inject_placeholders(br#"{"messages":[{"role":"user","content":"u"}]}"#).is_none());
        assert!(inject_placeholders(br#"{"input":"x"}"#).is_none());
        assert!(inject_placeholders(b"not json").is_none());
    }
}
