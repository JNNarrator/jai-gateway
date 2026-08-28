//! 上游模型发现 —— 需求 §2：按协议调用模型列表接口并归一。
//!
//! - openai_compat: GET {base}/models            (Bearer)
//! - anthropic    : GET {base}/v1/models         (x-api-key + anthropic-version)
//! - gemini       : GET {base}/v1beta/models     (x-goog-api-key，剥 models/ 前缀)
//!
//! Base URL 约定：openai_compat 填到 /v1 一级；anthropic/gemini 填主机根。

use serde_json::Value;
use std::time::Duration;

pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
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
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(|id| DiscoveredModel {
                    id: id.to_string(),
                    display_name: None,
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
                    Some(DiscoveredModel {
                        id: id.to_string(),
                        display_name: m
                            .get("displayName")
                            .and_then(Value::as_str)
                            .map(str::to_string),
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
