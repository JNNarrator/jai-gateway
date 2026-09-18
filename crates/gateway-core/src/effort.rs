//! 推理档位（reasoning effort）值域声明 —— 「这家上游认哪些值」的真相源。
//!
//! 背景（2026-09-18 真机故障，见 docs/zcode接入.md）：
//! zcode 走 JAI 的 Responses 入站时发 `reasoning:{"effort":"none"}`（其 provider 未
//! 声明推理能力时按「无推理」发 none），JAI 跨族转换把 `reasoning_effort:"none"`
//! **原样透传**给基元律动，而上游只认 low/medium/high/xhigh/max →
//! 400 `UNSUPPORTED_FIELD`，客户端只看到被包装过的 "Provider rejected the model
//! request."。根因不是模型名，而是「客户端参数值 ∈ 上游值域」这一步没人负责。
//!
//! 语义（对齐 docs/design/multimodal-support.md 的口径）：
//! - **存储**：TEXT 逗号串，**声明序即语义**（首项 = 自定义档位的默认档）；
//!   `NULL`/空串 = 未声明 ⇒ **不干预、原样透传**（向后兼容：上游自己认就认）。
//! - **归一**：[`place`] 把客户端值落到「透传 / 丢弃 / 映射」三选一。关闭语义
//!   （off/none/disabled）在该上游没有对应档位时**丢弃该参数**（不传 = 上游默认），
//!   而不是硬塞一个注定 400 的值。
//! - **不臆断**：未声明的供应商零改动（直通路径字节级不变）。

use serde_json::Value;

use crate::codec::Family;

/// 标准档位阶梯（小 → 大）。仅用于「未命中时选最近档」，与声明序无关。
/// 前三项是「关闭推理」的等价写法（各家拼写不一）。
const LADDER: [&str; 8] = [
    "off", "none", "disabled", "minimal", "low", "medium", "high", "xhigh",
];
/// 阶梯之上还有一档 `max`（= 阶梯顶端 +1）。
const MAX: &str = "max";

/// 关闭推理的等价写法。
pub fn is_off(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    v == "off" || v == "none" || v == "disabled"
}

/// 标准档位序号；自定义档位名（上游私有写法）→ `None`。
fn rank(value: &str) -> Option<usize> {
    let v = value.trim().to_ascii_lowercase();
    if v == MAX {
        return Some(LADDER.len());
    }
    LADDER.iter().position(|l| *l == v)
}

/// 解析声明值域：空白/大小写归一、保序去重；空串或全空 token → `None`（未声明）。
///
/// 与 [`crate::modality::parse`] 的差别：这里**不重排**（声明序携带语义），
/// 且**保留**无法归类的自定义 token（上游确有私有档位名）。
pub fn parse(s: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for tok in s.split(',') {
        let t = tok.trim().to_ascii_lowercase();
        if t.is_empty() || out.contains(&t) {
            continue;
        }
        out.push(t);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// `Option<&str>` 形态的 [`parse`]（DB 列读取直用）。
pub fn parse_opt(s: Option<&str>) -> Option<Vec<String>> {
    s.and_then(parse)
}

/// 编码为声明序逗号串。空集合 → 空串（读回即「未声明」）。
pub fn encode(levels: &[String]) -> String {
    levels.join(",")
}

/// 归一后编码（写库用）：空白/大小写/重复项归一；无任何有效档位 → `None`（未声明）。
pub fn encode_opt(levels: Option<&[String]>) -> Option<String> {
    levels.and_then(|l| parse(&encode(l))).map(|l| encode(&l))
}

/// JSON 数组（`["low","high"]`）→ 声明值域；非数组/空数组 → `None`。
pub fn from_json_array(v: &Value) -> Option<Vec<String>> {
    let arr = v.as_array()?;
    let mut out: Vec<String> = Vec::new();
    for x in arr {
        let Some(t) = x.as_str() else { continue };
        let t = t.trim().to_ascii_lowercase();
        if t.is_empty() || out.contains(&t) {
            continue;
        }
        out.push(t);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 客户端值相对声明值域的落点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// 值在域内（或域未声明）→ 原样透传。
    Passthrough,
    /// 关闭语义而域内无对应档位 → 丢弃该参数（不传 = 上游默认）。
    Drop,
    /// 值不在域内 → 映射到域内档位。
    Map(String),
}

/// 把客户端值落到「透传 / 丢弃 / 映射」。
///
/// 规则：
/// 1. 域未声明 → 透传（不干预）；
/// 2. 值在域内（大小写不敏感）→ 透传；
/// 3. 关闭语义（off/none/disabled）→ 丢弃（该上游无法表达「关掉推理」）；
/// 4. 标准档位但超/低于域 → 取域内**最近档**（并列取更保守的低档）；
/// 5. 其余（自定义档位名、域内无可比档位）→ 取**声明首项**（= 该供应商的默认档）。
pub fn place(value: &str, levels: &[String]) -> Placement {
    if levels.is_empty() {
        return Placement::Passthrough;
    }
    let v = value.trim();
    if levels.iter().any(|l| l.eq_ignore_ascii_case(v)) {
        return Placement::Passthrough;
    }
    if is_off(v) {
        return Placement::Drop;
    }
    match rank(v) {
        Some(r) => {
            let ranked: Vec<(usize, &String)> = levels
                .iter()
                .filter_map(|l| rank(l).map(|lr| (lr, l)))
                .collect();
            match ranked
                .iter()
                .min_by_key(|(lr, _)| (lr.abs_diff(r), *lr))
                .map(|(_, l)| (*l).clone())
            {
                Some(best) => Placement::Map(best),
                // 域内全是不可比的私有档位名 → 声明首项兜底
                None => Placement::Map(levels[0].clone()),
            }
        }
        None => Placement::Map(levels[0].clone()),
    }
}

/// 读取出站请求体里的推理档位（直通路径的「归一笔记」用）：
/// `openai_compat` 取顶层 `reasoning_effort`，`openai_responses` 取 `reasoning.effort`，
/// 其余族无此参数面 → `None`。
pub fn body_effort(body: &[u8], family: Family) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    match family {
        Family::OpenAiCompat => v
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .map(str::to_string),
        Family::OpenAiResponses => v
            .get("reasoning")
            .and_then(|r| r.get("effort"))
            .and_then(Value::as_str)
            .map(str::to_string),
        Family::Anthropic | Family::Gemini => None,
    }
}

/// 直通路径的 body 归一：把出站请求体里的推理档位改成「域内可接受」的写法。
///
/// 返回值语义：`None` = **无需改动（保持原始字节）**；`Some(bytes)` = 改写后的 body。
/// 只有「值存在且需要丢弃/映射」时才重编码，未声明值域的供应商零开销、零风险。
///
/// - `openai_compat`：顶层 `reasoning_effort`
/// - `openai_responses`：嵌套 `reasoning.effort`
/// - `anthropic` / `gemini`：无此参数面，直接不干预
pub fn normalize_body(body: &[u8], family: Family, levels: &[String]) -> Option<Vec<u8>> {
    if levels.is_empty() {
        return None;
    }
    let mut v: Value = serde_json::from_slice(body).ok()?;
    let cur = body_effort(body, family)?;

    match place(&cur, levels) {
        Placement::Passthrough => None,
        Placement::Drop => {
            match family {
                Family::OpenAiCompat => {
                    v.as_object_mut()?.remove("reasoning_effort");
                }
                Family::OpenAiResponses => {
                    let obj = v.as_object_mut()?;
                    if let Some(r) = obj.get_mut("reasoning").and_then(Value::as_object_mut) {
                        r.remove("effort");
                        // reasoning 只剩空壳（无 summary 等同伴键）→ 一并摘掉，
                        // 避免给上游塞 `"reasoning":{}` 这种可疑空对象
                        if r.is_empty() {
                            obj.remove("reasoning");
                        }
                    }
                }
                Family::Anthropic | Family::Gemini => return None,
            }
            serde_json::to_vec(&v).ok()
        }
        Placement::Map(next) => {
            match family {
                Family::OpenAiCompat => {
                    v.as_object_mut()?
                        .insert("reasoning_effort".into(), Value::String(next));
                }
                Family::OpenAiResponses => {
                    let r = v.get_mut("reasoning").and_then(Value::as_object_mut)?;
                    r.insert("effort".into(), Value::String(next));
                }
                Family::Anthropic | Family::Gemini => return None,
            }
            serde_json::to_vec(&v).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(s: &str) -> Vec<String> {
        parse(s).expect("测试值域必须非空")
    }

    #[test]
    fn parse_preserves_declaration_order_and_lenient_tokens() {
        assert_eq!(parse(""), None);
        assert_eq!(parse(" , "), None);
        // 保序 + 去重 + 大小写归一
        assert_eq!(lv("HIGH, low ,high"), vec!["high", "low"]);
        // 上游私有档位名保留（不臆断丢弃）
        assert_eq!(lv("thinking, off"), vec!["thinking", "off"]);
        assert_eq!(parse_opt(None), None);
    }

    #[test]
    fn encode_roundtrips() {
        let s = "low,medium,high,xhigh,max";
        assert_eq!(encode(&lv(s)), s);
    }

    #[test]
    fn from_json_array_is_lenient() {
        assert_eq!(
            from_json_array(&serde_json::json!(["LOW", "high", ""])),
            Some(vec!["low".into(), "high".into()])
        );
        assert_eq!(from_json_array(&serde_json::json!([])), None);
        assert_eq!(from_json_array(&serde_json::json!("low")), None);
    }

    #[test]
    fn undeclared_domain_never_intervenes() {
        assert_eq!(place("none", &[]), Placement::Passthrough);
        assert_eq!(
            normalize_body(br#"{"reasoning_effort":"none"}"#, Family::OpenAiCompat, &[]),
            None
        );
    }

    #[test]
    fn in_domain_passes_through() {
        let levels = lv("low,medium,high,xhigh,max");
        assert_eq!(place("high", &levels), Placement::Passthrough);
        assert_eq!(place("HIGH", &levels), Placement::Passthrough);
    }

    /// 本次真机故障的复刻：none 无对应档位 → 丢弃参数（而不是塞给上游换 400）。
    #[test]
    fn off_semantics_drops_when_unsupported() {
        let levels = lv("low,medium,high,xhigh,max");
        assert_eq!(place("none", &levels), Placement::Drop);
        assert_eq!(place("off", &levels), Placement::Drop);
        // 域内确实支持关闭时照常透传
        let with_off = lv("off,high,max");
        assert_eq!(place("none", &with_off), Placement::Drop); // none 与 off 拼写不同：域内没有 none
        assert_eq!(place("off", &with_off), Placement::Passthrough);
    }

    #[test]
    fn unranked_neighbour_maps_to_nearest() {
        let levels = lv("low,medium,high,xhigh,max");
        // minimal 低于域下限 → 收敛到最低档
        assert_eq!(place("minimal", &levels), Placement::Map("low".into()));
        // 域内本来就有的档位一律透传（不因「没见过」而改写）
        assert_eq!(place("xhigh", &levels), Placement::Passthrough);
        assert_eq!(place("max", &levels), Placement::Passthrough);
        // 域上限低于客户端诉求 → 收敛到域内最高档
        let capped = lv("low,medium,high");
        assert_eq!(place("xhigh", &capped), Placement::Map("high".into()));
        assert_eq!(place("max", &capped), Placement::Map("high".into()));
        assert_eq!(place("none", &capped), Placement::Drop);
    }

    #[test]
    fn private_level_names_fall_back_to_declared_default() {
        let levels = lv("thinking,non-thinking");
        assert_eq!(place("high", &levels), Placement::Map("thinking".into()));
        assert_eq!(place("non-thinking", &levels), Placement::Passthrough);
    }

    /// 关闭语义在「域内含 comparable 档位」时仍优先丢弃：
    /// 关掉推理是明确意图，映射成 low 反而会偷偷开推理。
    #[test]
    fn off_beats_nearest_mapping() {
        let levels = lv("low,high");
        assert_eq!(place("disabled", &levels), Placement::Drop);
    }

    #[test]
    fn normalize_body_compat_drops_field() {
        let body = br#"{"model":"deepseek-flash","reasoning_effort":"none","messages":[]}"#;
        let levels = lv("low,medium,high,xhigh,max");
        let out = normalize_body(body, Family::OpenAiCompat, &levels).expect("应改写");
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("reasoning_effort").is_none());
        assert_eq!(v["model"], "deepseek-flash");
        // 域内值 → 不改写（保原始字节）
        let ok = br#"{"model":"x","reasoning_effort":"high"}"#;
        assert_eq!(normalize_body(ok, Family::OpenAiCompat, &levels), None);
    }

    #[test]
    fn normalize_body_responses_drops_nested_effort_and_empty_shell() {
        let levels = lv("low,medium,high,xhigh,max");
        let body = br#"{"model":"x","reasoning":{"effort":"none"},"input":[]}"#;
        let out = normalize_body(body, Family::OpenAiResponses, &levels).expect("应改写");
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("reasoning").is_none(), "空壳 reasoning 应一并摘掉");
        // 有同伴键时只摘 effort
        let body2 = br#"{"model":"x","reasoning":{"effort":"none","summary":"auto"}}"#;
        let out2 = normalize_body(body2, Family::OpenAiResponses, &levels).unwrap();
        let v2: Value = serde_json::from_slice(&out2).unwrap();
        assert!(v2["reasoning"].get("effort").is_none());
        assert_eq!(v2["reasoning"]["summary"], "auto");
    }

    #[test]
    fn normalize_body_maps_and_ignores_other_families() {
        let levels = lv("low,medium,high,xhigh,max");
        let body = br#"{"model":"x","reasoning":{"effort":"minimal"}}"#;
        let out = normalize_body(body, Family::OpenAiResponses, &levels).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning"]["effort"], "low");

        let anthropic = br#"{"model":"x","reasoning_effort":"none"}"#;
        assert_eq!(normalize_body(anthropic, Family::Anthropic, &levels), None);
        // 非 JSON body 不炸
        assert_eq!(
            normalize_body(b"not-json", Family::OpenAiCompat, &levels),
            None
        );
    }
}
