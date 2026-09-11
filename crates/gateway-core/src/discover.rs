//! 上游模型发现 —— 需求 §2：按协议调用模型列表接口并归一。
//!
//! - openai_compat: GET {base}/models            (Bearer)
//! - anthropic    : GET {base}/v1/models         (x-api-key + anthropic-version)
//! - gemini       : GET {base}/v1beta/models     (x-goog-api-key，剥 models/ 前缀)
//!
//! Base URL 约定：openai_compat 填到 /v1 一级；anthropic/gemini 填主机根。

use serde_json::Value;
use std::time::Duration;

use crate::modality::{self, Modality};

pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
    /// 输入模态集合（0010）：None=上游未声明/未知，不做臆断。
    pub input_modalities: Option<Vec<Modality>>,
    /// 输出模态集合（0010）：None=上游未声明/未知。
    pub output_modalities: Option<Vec<Modality>>,
}

// ================================================================ 模态字段解析

/// openai 系 `/models` 项的输入/输出模态试探，按可信度递降：
/// 1. OpenRouter 风格 `architecture.input_modalities` / `architecture.output_modalities`（最规范）；
/// 2. 扁平 `input_modalities` / `inputModalities`（output 对称键同理）；
/// 3. 中转自定义的 `supports_vision` / `multimodal` / `vision`（布尔/数组/字符串）。
///
/// 全部取不到 → `(None, None)`（未知，不臆断；不做模型名启发式）。
pub fn parse_openai_modalities(m: &Value) -> (Option<Vec<Modality>>, Option<Vec<Modality>>) {
    let arch = m.get("architecture");
    let input = arch
        .and_then(|a| first_modalities(a, &["input_modalities", "inputModalities"]))
        .or_else(|| first_modalities(m, &["input_modalities", "inputModalities", "modalities"]))
        .or_else(|| legacy_vision_modalities(m));
    let output = arch
        .and_then(|a| first_modalities(a, &["output_modalities", "outputModalities"]))
        .or_else(|| first_modalities(m, &["output_modalities", "outputModalities"]));
    (input, output)
}

/// 在 scope 内按 key 顺序找第一个能解析成模态集合的字段。
fn first_modalities(scope: &Value, keys: &[&str]) -> Option<Vec<Modality>> {
    keys.iter()
        .find_map(|k| scope.get(*k).and_then(modality::from_json_array))
}

/// 旧 vision 键 → 输入模态：布尔 `true` ⇒ 文本+图像；`false` ⇒ 仅文本；
/// 数组/字符串按内容识别（含 vision/image/multimodal 关键词即视为图像）。
fn legacy_vision_modalities(m: &Value) -> Option<Vec<Modality>> {
    for key in ["supports_vision", "multimodal", "vision"] {
        let Some(v) = m.get(key) else { continue };
        match v {
            Value::Bool(true) => return Some(vec![Modality::Text, Modality::Image]),
            Value::Bool(false) => return Some(vec![Modality::Text]),
            Value::Array(arr) => {
                if let Some(list) = modality::from_json_array(v) {
                    return Some(list);
                }
                if arr
                    .iter()
                    .any(|x| x.as_str().map(vision_keyword).unwrap_or(false))
                {
                    return Some(vec![Modality::Text, Modality::Image]);
                }
            }
            // 字符串含 vision/image/multimodal 关键词即视为图像能力
            Value::String(s) if vision_keyword(s) => {
                return Some(vec![Modality::Text, Modality::Image])
            }
            _ => {}
        }
    }
    None
}

fn vision_keyword(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("vision") || lower.contains("image") || lower.contains("multimodal")
}

/// Gemini `/v1beta/models` 项：`inputModalities` / `outputModalities`
/// （大写枚举数组，最规范的一种）。无字段 → None（未知）。
pub fn parse_gemini_modalities(m: &Value) -> (Option<Vec<Modality>>, Option<Vec<Modality>>) {
    (
        m.get("inputModalities").and_then(modality::from_json_array),
        m.get("outputModalities")
            .and_then(modality::from_json_array),
    )
}

/// 发现失败返回 Err(摘要)（不含密钥）。
pub async fn discover_models(
    client: &reqwest::Client,
    family: &str,
    base_url: &str,
    secret: Option<&str>,
) -> Result<Vec<DiscoveredModel>, String> {
    match family {
        "openai_compat" | "openai_responses" => {
            let url = crate::codec::openai::url_join(base_url, "/models");
            let mut req = client.get(&url).timeout(DISCOVERY_TIMEOUT);
            if let Some(k) = secret {
                req = req.bearer_auth(k);
            }
            let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
            ensure_ok(resp.status(), &url).await?;
            let v: Value = resp
                .json()
                .await
                .map_err(|e| format!("JSON 解析失败: {e}"))?;
            let arr = v
                .get("data")
                .and_then(Value::as_array)
                .ok_or("响应缺少 data 数组")?;
            Ok(arr
                .iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(Value::as_str)?;
                    let (input_modalities, output_modalities) = parse_openai_modalities(m);
                    Some(DiscoveredModel {
                        id: id.to_string(),
                        display_name: None,
                        input_modalities,
                        output_modalities,
                    })
                })
                .collect())
        }
        "anthropic" => {
            let url = crate::codec::openai::url_join(base_url, "/v1/models");
            let mut req = client
                .get(&url)
                .timeout(DISCOVERY_TIMEOUT)
                .header("anthropic-version", ANTHROPIC_VERSION);
            if let Some(k) = secret {
                req = req.header("x-api-key", k);
            }
            let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
            ensure_ok(resp.status(), &url).await?;
            let v: Value = resp
                .json()
                .await
                .map_err(|e| format!("JSON 解析失败: {e}"))?;
            let arr = v
                .get("data")
                .and_then(Value::as_array)
                .ok_or("响应缺少 data")?;
            Ok(arr
                .iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(Value::as_str)?;
                    Some(DiscoveredModel {
                        id: id.to_string(),
                        display_name: m
                            .get("display_name")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        // Anthropic 官方 /v1/models 无模态标志，不做臆断
                        input_modalities: None,
                        output_modalities: None,
                    })
                })
                .collect())
        }
        "gemini" => {
            let url = crate::codec::openai::url_join(base_url, "/v1beta/models");
            let mut req = client.get(&url).timeout(DISCOVERY_TIMEOUT);
            if let Some(k) = secret {
                // storage §8-鉴权注记：头传递，避免 key 落入访问日志
                req = req.header("x-goog-api-key", k);
            }
            let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
            ensure_ok(resp.status(), &url).await?;
            let v: Value = resp
                .json()
                .await
                .map_err(|e| format!("JSON 解析失败: {e}"))?;
            let arr = v
                .get("models")
                .and_then(Value::as_array)
                .ok_or("响应缺少 models 数组")?;
            Ok(arr
                .iter()
                .filter_map(|m| {
                    let raw = m.get("name").and_then(Value::as_str)?;
                    let id = raw.strip_prefix("models/").unwrap_or(raw);
                    // 仅收支持 generateContent 的对话模型
                    let ok_method = m
                        .get("supportedGenerationMethods")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().any(|x| x.as_str() == Some("generateContent")))
                        .unwrap_or(true);
                    if !ok_method {
                        return None;
                    }
                    let (input_modalities, output_modalities) = parse_gemini_modalities(m);
                    Some(DiscoveredModel {
                        id: id.to_string(),
                        display_name: m
                            .get("displayName")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        input_modalities,
                        output_modalities,
                    })
                })
                .collect())
        }
        other => Err(format!("未知协议族: {other}")),
    }
}

async fn ensure_ok(status: reqwest::StatusCode, url: &str) -> Result<(), String> {
    if status.is_success() {
        return Ok(());
    }
    Err(format!(
        "{url} → HTTP {}（检查 Base URL 与 API Key；404 多为 base_url 少了或多了 /v1）",
        status.as_u16()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_image() -> (Option<Vec<Modality>>, Option<Vec<Modality>>) {
        (
            Some(vec![Modality::Text, Modality::Image]),
            Some(vec![Modality::Text]),
        )
    }

    #[test]
    fn openai_legacy_vision_bool_keys() {
        // 旧布尔键 → 文本+图像 / 纯文本
        assert_eq!(
            parse_openai_modalities(&json!({"supports_vision": true})).0,
            text_image().0
        );
        assert_eq!(
            parse_openai_modalities(&json!({"multimodal": false})).0,
            Some(vec![Modality::Text])
        );
        assert_eq!(
            parse_openai_modalities(&json!({"vision": true})).0,
            text_image().0
        );
    }

    #[test]
    fn openai_modality_arrays_and_architecture() {
        // 扁平数组
        assert_eq!(
            parse_openai_modalities(&json!({"multimodal": ["text", "image"]})).0,
            text_image().0
        );
        assert_eq!(
            parse_openai_modalities(&json!({"inputModalities": ["TEXT", "AUDIO"]})).0,
            Some(vec![Modality::Text, Modality::Audio])
        );
        // OpenRouter architecture 优先于扁平键
        assert_eq!(
            parse_openai_modalities(&json!({
                "input_modalities": ["text"],
                "architecture": {
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["text"]
                }
            })),
            text_image()
        );
    }

    #[test]
    fn openai_modalities_missing_is_unknown() {
        assert_eq!(
            parse_openai_modalities(&json!({"id": "gpt-4o"})),
            (None, None)
        );
        // 值类型不可用（数字/null）→ 跳过，继续试下一个 key
        assert_eq!(
            parse_openai_modalities(&json!({"supports_vision": 1, "vision": true})).0,
            text_image().0
        );
        assert_eq!(
            parse_openai_modalities(&json!({"supports_vision": null})),
            (None, None)
        );
    }

    #[test]
    fn gemini_input_output_modalities() {
        // 只声明输入 → 输出保持未知（不臆断为 text）
        assert_eq!(
            parse_gemini_modalities(&json!({"inputModalities": ["TEXT", "IMAGE"]})),
            (Some(vec![Modality::Text, Modality::Image]), None)
        );
        // 输入+输出都声明
        assert_eq!(
            parse_gemini_modalities(&json!({
                "inputModalities": ["TEXT", "IMAGE"],
                "outputModalities": ["TEXT"]
            })),
            text_image()
        );
        assert_eq!(
            parse_gemini_modalities(&json!({"inputModalities": ["TEXT"]})).0,
            Some(vec![Modality::Text])
        );
        // 无字段 → 未知
        assert_eq!(
            parse_gemini_modalities(&json!({"name": "models/gemini-1.5-pro"})),
            (None, None)
        );
    }
}
