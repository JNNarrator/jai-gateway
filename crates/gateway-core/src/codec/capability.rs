//! 能力声明与兼容性规划层（借鉴 GodeX bridge 的 ProviderSpec capabilities + planner）。
//!
//! 六面能力声明（parameters / tools / tool_choice / response_formats / reasoning /
//! streaming）+ 四级决策（supported / degraded / ignored / rejected）。
//!
//! - 能力表是**协议族级**静态声明（`caps_of`）；模型级差异（protocol-ir §8 注释的
//!   `target_model_capabilities`）是后续维度。
//! - `plan_compatibility` 只读请求产出决策；`CompatibilityPlan::resolve` 把决策
//!   应用到请求（降级指令注入 system、response_format 覆写）并判定拒绝。
//! - 拒绝保持网关现有 400 语义（`wire.error_response` + 错误码），Lenient 字段
//!   维持 WARN 汇总（原 `extension_warn_note` 通道收敛于此）。

use crate::codec::ir::{CanonicalRequest, ToolChoice};
use crate::codec::Family;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::LazyLock;

// ================================================================ 能力声明

/// reasoning effort 能力档位（GodeX：none / boolean / native）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortMode {
    /// 无思考控制（参数被忽略）
    None,
    /// 仅开/关（如 Anthropic thinking enabled/disabled）
    Boolean,
    /// 原生力度档位（如 OpenAI reasoning_effort）
    Native,
}

/// 无法原生表达响应格式时的降级执行方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatDegradation {
    /// 降级为 `json_object` 上游参数 + 指令注入（chat 系上游）
    JsonObject,
    /// 仅提示词指令注入（无响应格式参数的协议族）
    InstructionOnly,
}

/// 出站协议族的能力声明。
#[derive(Debug)]
pub struct Capabilities {
    /// 该能力表对应的出站协议族（规划层据此决定哪些「渠道策略」对该族有意义）
    pub family: Family,
    /// 支持的请求参数名（文档性声明；SampleParams 各字段由 encoder 负责映射）
    pub parameters: HashSet<&'static str>,
    /// 支持的工具声明类型（IR `ToolSpec` 目前只有 function）
    pub tools: HashSet<&'static str>,
    /// 无法原生表达的工具类型 → 降级为 function（键=请求类型，值=降级目标类型）
    pub tools_degraded: &'static [(&'static str, &'static str)],
    /// 该协议族的工具声明上限（超限 Rejected → `tools_limit_exceeded`）。
    ///
    /// **四族一律 `None`**：协议本身没有这条限制，网关曾按 128 硬拦（bug 24）——
    /// 2026-09-18 实测上游接受 140 / 300 个工具均 200，而 zcode 的 140 个工具被
    /// JAI 自己 400 掉，误杀了上游完全能跑的配置。上限只来自**渠道声明**
    /// （[`ChannelPolicy::max_tools`]，供应商/模型级可配），未声明即放行由上游裁决。
    pub max_tools: Option<usize>,
    /// 支持的 tool_choice 模式名（auto / none / required / specific）
    pub tool_choice: HashSet<&'static str>,
    /// 原生支持的响应格式（text / json_object / json_schema）
    pub response_formats: HashSet<&'static str>,
    /// 无法原生表达的响应格式 → 降级方式（键=请求格式）
    pub degraded_formats: &'static [(&'static str, FormatDegradation)],
    /// reasoning effort 能力
    pub reasoning: EffortMode,
    /// 流式 usage 支持（encoder 决定是否注入 stream_options/include_usage）
    pub streaming_usage: bool,
    /// 工具结果内嵌图片能否原生承载（`tool_result` / `function_call_output` 里的图片）。
    ///
    /// `false` 时 encoder 会把它**降级提升**为紧随其后的一条 user 消息
    /// （两族都原生支持 user 消息带图），并由本层记一条 CapabilityWarn——
    /// 而不是像旧实现那样静默丢弃。
    pub tool_result_images: bool,
}

fn set(ss: &[&'static str]) -> HashSet<&'static str> {
    ss.iter().copied().collect()
}

/// Codex 扩展工具类型 → function 降级（所有出站族统一；详见 §10 降级矩阵）。
/// 折叠名：shell / apply_patch / local_shell 用固定名，custom 用原名（重名加后缀）。
const EXTENDED_TOOLS_DEGRADED: &[(&str, &str)] = &[
    ("shell", "function"),
    ("apply_patch", "function"),
    ("custom", "function"),
    ("local_shell", "function"),
];

static OPENAI_COMPAT_CAPS: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    family: Family::OpenAiCompat,
    parameters: set(&[
        "stream",
        "temperature",
        "top_p",
        "max_output_tokens",
        "stop",
        "frequency_penalty",
        "presence_penalty",
        "seed",
        "reasoning_effort",
        "response_format",
    ]),
    tools: set(&["function"]),
    tools_degraded: EXTENDED_TOOLS_DEGRADED,
    max_tools: None,
    tool_choice: set(&["auto", "none", "required", "specific"]),
    response_formats: set(&["text", "json_object", "json_schema"]),
    degraded_formats: &[],
    reasoning: EffortMode::Native,
    streaming_usage: true,
    // OpenAI chat 的 role=tool 消息 content 只能是字符串，装不下图片
    tool_result_images: false,
});

static OPENAI_RESPONSES_CAPS: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    family: Family::OpenAiResponses,
    parameters: set(&[
        "stream",
        "temperature",
        "top_p",
        "max_output_tokens",
        "stop",
        "reasoning_effort",
        "response_format",
    ]),
    tools: set(&["function"]),
    tools_degraded: EXTENDED_TOOLS_DEGRADED,
    max_tools: None,
    tool_choice: set(&["auto", "none", "required", "specific"]),
    response_formats: set(&["text", "json_object", "json_schema"]),
    degraded_formats: &[],
    reasoning: EffortMode::Native,
    streaming_usage: true,
    // Responses 的 function_call_output.output 支持内容项数组（含 input_image）
    tool_result_images: true,
});

static ANTHROPIC_CAPS: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    family: Family::Anthropic,
    parameters: set(&["max_output_tokens", "temperature", "top_p", "top_k", "stop"]),
    tools: set(&["function"]),
    tools_degraded: EXTENDED_TOOLS_DEGRADED,
    max_tools: None,
    tool_choice: set(&["auto", "none", "required", "specific"]),
    response_formats: set(&["text"]),
    degraded_formats: &[
        ("json_object", FormatDegradation::InstructionOnly),
        ("json_schema", FormatDegradation::InstructionOnly),
    ],
    reasoning: EffortMode::Boolean,
    streaming_usage: true,
    // Anthropic 的 tool_result.content 是 content block 数组，可含 image
    tool_result_images: true,
});

static GEMINI_CAPS: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    family: Family::Gemini,
    parameters: set(&[
        "max_output_tokens",
        "temperature",
        "top_p",
        "top_k",
        "stop",
        "seed",
    ]),
    tools: set(&["function"]),
    tools_degraded: EXTENDED_TOOLS_DEGRADED,
    max_tools: None,
    tool_choice: set(&["auto", "none", "required", "specific"]),
    response_formats: set(&["text"]),
    degraded_formats: &[
        ("json_object", FormatDegradation::InstructionOnly),
        ("json_schema", FormatDegradation::InstructionOnly),
    ],
    reasoning: EffortMode::None,
    streaming_usage: false,
    // v1beta 的 functionResponse.parts 可承载 inlineData（JAI 出站固定打 v1beta）
    tool_result_images: true,
});

/// 按出站协议族取能力表。
pub fn caps_of(family: Family) -> &'static Capabilities {
    match family {
        Family::OpenAiCompat => &OPENAI_COMPAT_CAPS,
        Family::OpenAiResponses => &OPENAI_RESPONSES_CAPS,
        Family::Anthropic => &ANTHROPIC_CAPS,
        Family::Gemini => &GEMINI_CAPS,
    }
}

// ================================================================ 决策模型

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionAction {
    Supported,
    Degraded,
    Ignored,
    Rejected,
}

impl DecisionAction {
    pub fn is_rejected(&self) -> bool {
        matches!(self, DecisionAction::Rejected)
    }
}

/// 单参数/单能力的兼容性决策。
#[derive(Debug, Clone)]
pub struct Decision {
    pub action: DecisionAction,
    pub reason: String,
}

/// 规划诊断（供 WARN 汇总与测试断言）。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub path: String,
    pub action: DecisionAction,
    pub reason: String,
}

/// 结构化输出的执行契约（mode：原生外传 / 降级执行）。
#[derive(Debug, Clone)]
pub struct OutputContract {
    /// 覆写 `extensions["response_format"]` 的值（None = 沿用入站原值，encoder 外传）
    pub provider_format: Option<Value>,
    /// 降级指令（append 到 `system` 末条，各 encoder 天然带出）
    pub instruction: Option<String>,
    /// 非流式响应需做 JSON 校验（仅降级路径）
    pub validate_output: bool,
}

/// 某条渠道声明的「推理档位值域」（0011，见 `crate::effort`）。
///
/// 族级能力表（[`Capabilities::reasoning`]）只回答「这一族支不支持原生 effort」；
/// 值域回答「这家上游具体认哪些写法」——两者互补：前者管映射方式，后者管取值。
#[derive(Debug, Clone)]
pub struct EffortPolicy {
    /// 声明序值域（首项 = 自定义档位的默认档）
    pub levels: Vec<String>,
    /// WARN 文案用的来源描述（如 `供应商「基元律动」`）
    pub source: String,
}

impl EffortPolicy {
    /// 值域为空视同未声明（返回 None），避免调用方各写一遍判空。
    pub fn new(levels: Option<&Vec<String>>, source: impl Into<String>) -> Option<Self> {
        let levels = levels.filter(|l| !l.is_empty())?.clone();
        Some(EffortPolicy {
            levels,
            source: source.into(),
        })
    }
}

/// 单条渠道（供应商/模型）的**声明式覆盖**集合。
///
/// 两个成员遵循同一哲学：**没声明就不干预**（网关不发明限制），**声明了就严格执行**。
/// 之所以需要这层：族级能力表描述「协议本身能不能」，而「这家上游实际认什么」
/// 只有渠道自己知道（上游网关各自的校验规则不同）。
#[derive(Debug, Clone, Default)]
pub struct ChannelPolicy {
    /// 推理档位值域（0011，见 `crate::effort`）
    pub effort: Option<EffortPolicy>,
    /// 工具声明数上限（0012）：None = 未声明 ⇒ 不拦，由上游裁决
    pub max_tools: Option<usize>,
    /// 该渠道是否需要「非空推理回放」（`crate::codec::replay`）：
    /// 推断命中或学习命中时为 true。仅在目标族是 openai_compat 时落地
    /// （推理字段只存在于 chat-completions 线上）。
    pub reasoning_replay: bool,
}

impl ChannelPolicy {
    /// 三项都没声明 → None（调用方可借此整段短路，保持零干预快路径）。
    pub fn is_empty(&self) -> bool {
        self.effort.is_none() && self.max_tools.is_none() && !self.reasoning_replay
    }
}

/// 规划对 `reasoning_effort` 的落地动作（resolve 阶段真正改写请求）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EffortRewrite {
    /// 保持入站值（域内可接受，或该族本来就不用 effort）
    #[default]
    Keep,
    /// 丢弃参数（上游无法表达该诉求，不传 = 上游默认）
    Clear,
    /// 改写为域内档位
    Set(String),
}

/// 一次请求在目标族的规划结果。
#[derive(Debug)]
pub struct CompatibilityPlan<'a> {
    pub capabilities: &'a Capabilities,
    pub diagnostics: Vec<Diagnostic>,
    pub response_format: Option<Decision>,
    pub reasoning: Option<Decision>,
    pub tool_choice: Option<Decision>,
    pub tools: Option<Decision>,
    pub output_contract: Option<OutputContract>,
    /// 扩展工具折叠的身份映射（resolve 时写入 extensions[TOOL_IDENTITIES_KEY]）
    pub tool_identities: Option<Vec<Value>>,
    /// 推理档位改写动作（0011；`Keep` = 不动）
    pub reasoning_rewrite: EffortRewrite,
    /// 该渠道需要「非空推理回放」（`crate::codec::replay`）：resolve 时写入 extensions
    pub reasoning_replay: bool,
}

/// 规划应用产物：拒绝错误 / WARN 汇总 / 请求改写已完成。
#[derive(Debug, Default)]
pub struct PlanOutcome {
    /// (客户端消息, 错误码)。错误码 None 时 `wire.error_response` 传 None。
    pub rejection: Option<(String, Option<&'static str>)>,
    /// 每条一条原因的 WARN（Ignored / Degraded），proxy 合并后 eprintln。
    pub warnings: Vec<String>,
}

// ================================================================ 规划

/// 面向目标协议族规划一次请求的兼容性（不声明推理档位值域，值原样透传）。
pub fn plan_compatibility<'a>(
    req: &CanonicalRequest,
    caps: &'a Capabilities,
) -> CompatibilityPlan<'a> {
    plan_compatibility_with(req, caps, None)
}

/// 带渠道声明（[`ChannelPolicy`]：推理档位值域 0011 + 工具数上限 0012）的规划入口。
///
/// `policy` 为 None 时与 [`plan_compatibility`] 完全等价（未声明的供应商
/// 行为字节级不变）。
pub fn plan_compatibility_with<'a>(
    req: &CanonicalRequest,
    caps: &'a Capabilities,
    policy: Option<&ChannelPolicy>,
) -> CompatibilityPlan<'a> {
    let mut plan = CompatibilityPlan {
        capabilities: caps,
        diagnostics: Vec::new(),
        response_format: None,
        reasoning: None,
        tool_choice: None,
        tools: None,
        output_contract: None,
        tool_identities: None,
        reasoning_rewrite: EffortRewrite::Keep,
        reasoning_replay: policy.is_some_and(|p| p.reasoning_replay),
    };

    plan_response_format(req, caps, &mut plan);
    plan_reasoning(req, caps, policy, &mut plan);
    plan_tools(req, caps, policy, &mut plan);
    plan_tool_choice(req, &mut plan);
    plan_extension_fields(req, caps, &mut plan);
    plan_tool_result_images(req, caps, &mut plan);

    plan
}

/// 工具结果内嵌图片的承载能力。
///
/// agent 客户端（Claude Code / dsh / Codex）的截图类工具把图片放在**工具结果**里；
/// 目标族装不下时（如 OpenAI chat 的 `role=tool` 只允许 text part），encoder 会把它
/// **降级提升**为紧随其后的一条 user 消息——这里是该降级的显式记录点，
/// 避免像旧实现那样静默丢图（旧实现只渲染文本块，图片无声消失）。
fn plan_tool_result_images(
    req: &CanonicalRequest,
    caps: &Capabilities,
    plan: &mut CompatibilityPlan,
) {
    if caps.tool_result_images || !crate::codec::image::request_has_tool_result_image(req) {
        return;
    }
    plan.diagnostics.push(Diagnostic {
        path: "messages[].tool_result.image".into(),
        action: DecisionAction::Degraded,
        reason: "工具结果内嵌图片在该上游族无法原生承载，已降级提升为紧随其后的一条 user 消息"
            .into(),
    });
}

fn plan_response_format(req: &CanonicalRequest, caps: &Capabilities, plan: &mut CompatibilityPlan) {
    let Some(format) = req.extensions.get("response_format") else {
        return;
    };
    let Some(r#type) = format.get("type").and_then(Value::as_str) else {
        plan.response_format = Some(Decision {
            action: DecisionAction::Rejected,
            reason: "response_format 缺少 type 字段".into(),
        });
        return;
    };

    if caps.response_formats.contains(r#type) {
        plan.response_format = Some(Decision {
            action: DecisionAction::Supported,
            reason: format!("{type} 由上游原生支持"),
        });
        return;
    }

    let degraded = caps
        .degraded_formats
        .iter()
        .find(|(key, _)| *key == r#type)
        .map(|(_, mode)| *mode);
    match degraded {
        Some(FormatDegradation::JsonObject) => {
            plan.response_format = Some(Decision {
                action: DecisionAction::Degraded,
                reason: format!("{type} 降级为 json_object + 提示词约束"),
            });
            plan.output_contract = Some(OutputContract {
                provider_format: Some(json!({"type": "json_object"})),
                instruction: Some(build_json_instruction(format)),
                validate_output: r#type == "json_schema" && format_strict(format),
            });
        }
        Some(FormatDegradation::InstructionOnly) => {
            plan.response_format = Some(Decision {
                action: DecisionAction::Degraded,
                reason: format!("{type} 降级为提示词约束（上游无响应格式参数）"),
            });
            plan.output_contract = Some(OutputContract {
                provider_format: None,
                instruction: Some(build_json_instruction(format)),
                validate_output: r#type == "json_schema" && format_strict(format),
            });
        }
        None => {
            plan.response_format = Some(Decision {
                action: DecisionAction::Rejected,
                reason: format!(
                    "response_format({type}) 在跨协议转换中不支持；支持子集：{}",
                    caps.response_formats
                        .iter()
                        .copied()
                        .collect::<Vec<_>>()
                        .join("/")
                ),
            });
        }
    }
}

fn plan_reasoning(
    req: &CanonicalRequest,
    caps: &Capabilities,
    policy: Option<&ChannelPolicy>,
    plan: &mut CompatibilityPlan,
) {
    let Some(effort) = &req.params.reasoning_effort else {
        return;
    };
    // 原生 effort 族：先按渠道声明的值域归位（0011）。
    // 未声明值域的渠道与旧行为完全一致（原样透传）。
    if caps.reasoning == EffortMode::Native {
        if let Some(policy) = policy.and_then(|p| p.effort.as_ref()) {
            match crate::effort::place(effort, &policy.levels) {
                crate::effort::Placement::Passthrough => {
                    plan.reasoning = Some(Decision {
                        action: DecisionAction::Supported,
                        reason: format!("reasoning.effort({effort}) 原生透传"),
                    });
                }
                crate::effort::Placement::Drop => {
                    plan.reasoning_rewrite = EffortRewrite::Clear;
                    plan.reasoning = Some(Decision {
                        action: DecisionAction::Ignored,
                        reason: format!(
                            "reasoning.effort({effort})：{}声明档位为 {}，无法表达「关闭推理」→ 已丢弃该参数按上游默认处理",
                            policy.source,
                            policy.levels.join("/")
                        ),
                    });
                }
                crate::effort::Placement::Map(next) => {
                    plan.reasoning_rewrite = EffortRewrite::Set(next.clone());
                    plan.reasoning = Some(Decision {
                        action: DecisionAction::Degraded,
                        reason: format!(
                            "reasoning.effort({effort})：{}声明档位为 {}，已改写为 {next}",
                            policy.source,
                            policy.levels.join("/")
                        ),
                    });
                }
            }
            return;
        }
    }
    let decision = match caps.reasoning {
        EffortMode::Native => Decision {
            action: DecisionAction::Supported,
            reason: format!("reasoning.effort({effort}) 原生透传"),
        },
        EffortMode::Boolean => Decision {
            action: DecisionAction::Degraded,
            reason: format!("reasoning.effort({effort}) 映射为 thinking 开/关"),
        },
        EffortMode::None => Decision {
            action: DecisionAction::Ignored,
            reason: format!("reasoning.effort({effort}) 不支持，已忽略"),
        },
    };
    plan.reasoning = Some(decision);
}

/// 工具身份映射在请求扩展里的内部键（折叠的扩展工具还原用，§10）。
pub const TOOL_IDENTITIES_KEY: &str = "__jai_tool_identities";

/// 单个工具的原始类型身份（shell/apply_patch/custom/local_shell → function 折叠）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    pub requested_type: String,
    pub requested_name: String,
    /// 折叠后的 function 名（上游见到的名字）
    pub provider_name: String,
}

/// 从请求扩展里读工具身份映射（无则空）。
pub fn tool_identities_of(req: &CanonicalRequest) -> Vec<ToolIdentity> {
    let Some(Value::Array(arr)) = req.extensions.get(TOOL_IDENTITIES_KEY) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            Some(ToolIdentity {
                requested_type: v.get("requested_type")?.as_str()?.to_string(),
                requested_name: v.get("requested_name")?.as_str()?.to_string(),
                provider_name: v.get("provider_name")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// 按上游收到的 function 名还原请求侧工具类型（默认 function_call）。
/// 命中扩展工具身份 → 原始类型；固定名（shell/apply_patch/local_shell）兜底。
pub fn restore_tool_type(provider_name: &str, identities: &[ToolIdentity]) -> &'static str {
    if let Some(id) = identities.iter().find(|i| i.provider_name == provider_name) {
        return match id.requested_type.as_str() {
            "shell" => "shell_call",
            "apply_patch" => "apply_patch_call",
            "custom" => "custom_tool_call",
            "local_shell" => "local_shell_call",
            _ => "function_call",
        };
    }
    match provider_name {
        "shell" => "shell_call",
        "apply_patch" => "apply_patch_call",
        "local_shell" => "local_shell_call",
        _ => "function_call",
    }
}

/// 直通路径用：数出请求体里声明的工具个数（`tools` 数组长度）。
///
/// `openai_compat` 与 `openai_responses` 的键同名（都是顶层 `tools`）；
/// 无该键 / 非 JSON → `None`（无从判定，不拦）。
pub fn body_tool_count(body: &[u8]) -> Option<usize> {
    let v: Value = serde_json::from_slice(body).ok()?;
    v.get("tools")?.as_array().map(|a| a.len())
}

fn plan_tools(
    req: &CanonicalRequest,
    caps: &Capabilities,
    policy: Option<&ChannelPolicy>,
    plan: &mut CompatibilityPlan,
) {
    // 生效上限：渠道声明优先，其次族级（四族目前均为 None ⇒ 不拦，由上游裁决）。
    let max = policy.and_then(|p| p.max_tools).or(caps.max_tools);
    if let Some(max) = max {
        if req.tools.len() > max {
            plan.tools = Some(Decision {
                action: DecisionAction::Rejected,
                reason: format!(
                    "工具声明数 {} 超过该渠道声明上限 {max}（可在供应商/模型设置里调整该上限）",
                    req.tools.len()
                ),
            });
            return;
        }
    }

    // 扩展工具折叠：为声明分配 provider 名（固定名或原名，重名加后缀）并记身份。
    let mut used = std::collections::HashSet::new();
    let mut identities: Vec<ToolIdentity> = Vec::new();
    for t in &req.tools {
        let requested_type = match t.name.as_str() {
            "shell" => "shell",
            "apply_patch" => "apply_patch",
            "local_shell" => "local_shell",
            _ => "function", // 普通 function 工具
        };
        let is_extended = caps
            .tools_degraded
            .iter()
            .any(|(ty, _)| *ty == requested_type);
        if !is_extended {
            continue;
        }
        let base = match requested_type {
            "shell" => "shell",
            "apply_patch" => "apply_patch",
            "local_shell" => "local_shell",
            _ => t.name.as_str(), // custom
        };
        let provider_name = alloc_name(base, &mut used);
        identities.push(ToolIdentity {
            requested_type: requested_type.to_string(),
            requested_name: t.name.clone(),
            provider_name,
        });
    }
    if !identities.is_empty() {
        let arr: Vec<Value> = identities
            .iter()
            .map(|i| {
                json!({
                    "requested_type": i.requested_type,
                    "requested_name": i.requested_name,
                    "provider_name": i.provider_name,
                })
            })
            .collect();
        plan.tool_identities = Some(arr);
    }

    plan.tools = Some(Decision {
        action: DecisionAction::Supported,
        reason: format!("{} 个工具声明", req.tools.len()),
    });
}

fn alloc_name(base: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut i = 2;
    loop {
        let candidate = format!("{base}_{i}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        i += 1;
    }
}

fn plan_tool_choice(req: &CanonicalRequest, plan: &mut CompatibilityPlan) {
    match &req.tool_choice {
        ToolChoice::Specific(name) => {
            let declared = req.tools.iter().any(|t| &t.name == name);
            plan.tool_choice = Some(Decision {
                action: if declared {
                    DecisionAction::Supported
                } else {
                    DecisionAction::Rejected
                },
                reason: if declared {
                    format!("tool_choice 指定工具 {name} 已声明")
                } else {
                    format!("显式 tool_choice 指定工具 {name} 未在本请求声明，无法满足")
                },
            });
        }
        other => {
            plan.tool_choice = Some(Decision {
                action: DecisionAction::Supported,
                reason: format!("tool_choice({other:?}) 由上游原生支持"),
            });
        }
    }
}

/// 未建模扩展字段与能力面字段（Lenient WARN / 显式拒绝）。
/// decode 层只做结构解析，能力拒绝集中在这里（Phase B 收敛 n>1/logprobs 于此）。
fn plan_extension_fields(
    req: &CanonicalRequest,
    _caps: &Capabilities,
    plan: &mut CompatibilityPlan,
) {
    // 能力面字段 → 拒绝原因（返回 None 表示值不触发拒绝，如 n==1）
    type Judge = fn(&Value) -> Option<String>;
    for (key, reason) in [
        ("n", reject_reason_n as Judge),
        ("logprobs", reject_reason_present as Judge),
        ("top_logprobs", reject_reason_present as Judge),
    ] {
        if let Some(v) = req.extensions.get(key) {
            if let Some(reason) = reason(v) {
                plan.diagnostics.push(Diagnostic {
                    path: key.to_string(),
                    action: DecisionAction::Rejected,
                    reason,
                });
            }
        }
    }

    let handled: &[&str] = &[
        "response_format",
        "reasoning",
        "n",
        "logprobs",
        "top_logprobs",
        TOOL_IDENTITIES_KEY,
    ];
    // 协议标准字段但转换目标无对应语义（或语义天然满足）——丢弃属预期，静默不告警。
    // 例如 Responses 的 store（请求服务端存储会话，转发网关不代管）、
    // parallel_tool_calls（chat 上游天然并行）、metadata/user/include/previous_response_id。
    // 防止常规客户端字段刷 CapabilityWarn 淹没真正的降级告警（§10 观测性）。
    const KNOWN_SILENT: &[&str] = &[
        "store",
        "parallel_tool_calls",
        "stream_options",
        "metadata",
        "user",
        "include",
        "previous_response_id",
    ];
    for key in req.extensions.keys() {
        if handled.contains(&key.as_str()) || KNOWN_SILENT.contains(&key.as_str()) {
            continue;
        }
        plan.diagnostics.push(Diagnostic {
            path: key.clone(),
            action: DecisionAction::Ignored,
            reason: format!("{key} 未建模，已按 Lenient 丢弃"),
        });
    }
}

/// `n` 值 > 1 时拒绝（n==1 是默认值，允许）。
fn reject_reason_n(v: &Value) -> Option<String> {
    match v.as_u64() {
        Some(n) if n > 1 => Some("n>1（多候选）跨协议转换不支持；请使用 n=1".into()),
        _ => None,
    }
}

/// 字段存在即拒绝（logprobs / top_logprobs）。
fn reject_reason_present(_v: &Value) -> Option<String> {
    Some("logprobs/top_logprobs 跨协议转换不支持".into())
}

// ================================================================ 应用

impl<'a> CompatibilityPlan<'a> {
    /// 应用规划：改写入站请求（降级指令进 system、response_format 覆写），
    /// 并产出拒绝 / WARN 汇总。请求按引用改写，拒绝时可能已有部分改写——
    /// proxy 侧拒绝即 400 返回，不继续编码，改写无副作用。
    pub fn resolve(mut self, req: &mut CanonicalRequest) -> PlanOutcome {
        // 1) 汇总 WARN（Ignored / Degraded 各一条原因）——须在 take 清空决策字段之前
        let mut warnings: Vec<String> = Vec::new();
        for diag in &self.diagnostics {
            if matches!(
                diag.action,
                DecisionAction::Ignored | DecisionAction::Degraded
            ) {
                warnings.push(diag.reason.clone());
            }
        }
        for decision in [
            self.response_format.as_ref(),
            self.reasoning.as_ref(),
            self.tool_choice.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if matches!(
                decision.action,
                DecisionAction::Degraded | DecisionAction::Ignored
            ) {
                warnings.push(decision.reason.clone());
            }
        }

        // 2) 应用推理档位改写（0011）：必须在编码之前落地 —— 各 encoder 只读
        //    `params.reasoning_effort`，改写在这里发生，encoder 侧零改动。
        match std::mem::take(&mut self.reasoning_rewrite) {
            EffortRewrite::Keep => {}
            EffortRewrite::Clear => req.params.reasoning_effort = None,
            EffortRewrite::Set(next) => req.params.reasoning_effort = Some(next),
        }

        // 3) 拒绝优先：tools 超限 > response_format > tool_choice（首个 Rejected 即终止）
        for (path, decision) in [
            ("tools", self.tools.take()),
            ("response_format", self.response_format.take()),
            ("tool_choice", self.tool_choice.take()),
        ] {
            if let Some(dec) = decision {
                if dec.action.is_rejected() {
                    let code = match path {
                        "tools" => Some("tools_limit_exceeded"),
                        "response_format" => Some("response_format_not_supported"),
                        "tool_choice" => Some("tool_choice_not_supported"),
                        _ => None,
                    };
                    return PlanOutcome {
                        rejection: Some((dec.reason, code)),
                        warnings: Vec::new(),
                    };
                }
            }
        }

        // 3) diagnostics 里的 Rejected（n>1 / logprobs 等）→ 拒绝，错误码沿用 decode 现状（None）
        for diag in &self.diagnostics {
            if diag.action.is_rejected() {
                return PlanOutcome {
                    rejection: Some((diag.reason.clone(), None)),
                    warnings: Vec::new(),
                };
            }
        }

        // 4) 应用输出契约（降级指令进 system、response_format 覆写、校验标志）
        if let Some(contract) = self.output_contract {
            if let Some(format) = contract.provider_format {
                req.extensions.insert("response_format".into(), format);
            }
            if let Some(instruction) = contract.instruction {
                // 合并进末条 system 而非 push 新条——anthropic 出站要求 system 单条
                match req.system.last_mut() {
                    Some(last) => {
                        last.push('\n');
                        last.push_str(&instruction);
                    }
                    None => req.system.push(instruction),
                }
            }
            if contract.validate_output {
                // 校验标志随请求面走：proxy 非流式路径据此做输出 JSON 校验
                req.extensions
                    .insert("__jai_validate_output".into(), json!(true));
            }
        }

        // 5) 扩展工具折叠身份映射写入请求（渲染侧还原用，见 §10）
        if let Some(identities) = self.tool_identities {
            req.extensions
                .insert(TOOL_IDENTITIES_KEY.into(), Value::Array(identities));
        }

        // 6) 推理回放兼容（自适应，`crate::codec::replay`）：把「该渠道要求非空推理」
        //    告诉 openai encoder。走 extensions 而不是改 encoder 签名 —— 与 5) 同一套机制。
        //    只在目标族是 openai_compat 时落地：推理字段只存在于 chat-completions 线上，
        //    对 anthropic / gemini 出站塞这个标记没有意义。
        if self.reasoning_replay && self.capabilities.family == Family::OpenAiCompat {
            req.extensions
                .insert(crate::codec::replay::EXT_KEY.into(), json!(true));
        }

        PlanOutcome {
            rejection: None,
            warnings,
        }
    }
}

// ================================================================ 输出契约

/// 生成 GodeX 风格的结构化输出降级指令。
/// `format` 为入站 `response_format` 原值（OpenAI 形状：
/// `{type, json_schema?: {name, description, schema, strict}}`）。
pub fn build_json_instruction(format: &Value) -> String {
    let json_schema = format.get("json_schema").cloned().unwrap_or(Value::Null);
    let mut lines: Vec<String> = Vec::new();
    if let Some(name) = json_schema.get("name").and_then(Value::as_str) {
        lines.push(format!("Schema name: {name}"));
    }
    if let Some(description) = json_schema.get("description").and_then(Value::as_str) {
        lines.push(format!("Schema description: {description}"));
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push("Return only valid JSON.".into());
    lines.push(String::new());
    lines.push("Rules:".into());
    lines.push("- Output exactly one JSON value and nothing else.".into());
    lines.push("- Do not include markdown, code fences, explanations, or extra text.".into());
    if let Some(schema) = json_schema.get("schema") {
        lines.push("- Use the JSON Schema below as formatting guidance.".into());
        lines.push(String::new());
        lines.push("JSON Schema:".into());
        lines.push(serde_json::to_string_pretty(schema).unwrap_or_else(|_| "null".into()));
    }
    if format_strict(format) {
        lines.push(String::new());
        lines.push(
            "Final output format override: return exactly one valid JSON object matching the \
             requested schema. This overrides any prior request for plain text, markdown, or \
             extra text."
                .into(),
        );
    }
    lines.join("\n")
}

fn format_strict(format: &Value) -> bool {
    format
        .get("json_schema")
        .and_then(|s| s.get("strict"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

// ================================================================ 测试

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn req_with_extensions(ext: Map<String, Value>) -> CanonicalRequest {
        let mut req = CanonicalRequest {
            model: "m".into(),
            ..Default::default()
        };
        req.extensions = ext;
        req
    }

    fn response_format_json_schema() -> Value {
        json!({
            "type": "json_schema",
            "json_schema": {
                "name": "weather",
                "description": "weather reply",
                "schema": {"type": "object", "properties": {"temp": {"type": "number"}}},
                "strict": true
            }
        })
    }

    #[test]
    fn caps_of_families() {
        assert_eq!(caps_of(Family::OpenAiCompat).reasoning, EffortMode::Native);
        assert_eq!(
            caps_of(Family::OpenAiResponses).reasoning,
            EffortMode::Native
        );
        assert_eq!(caps_of(Family::Anthropic).reasoning, EffortMode::Boolean);
        assert_eq!(caps_of(Family::Gemini).reasoning, EffortMode::None);
        assert!(caps_of(Family::OpenAiCompat)
            .response_formats
            .contains("json_schema"));
        assert!(caps_of(Family::Anthropic).response_formats.contains("text"));
        assert!(!caps_of(Family::Anthropic)
            .response_formats
            .contains("json_object"));
        for f in [
            Family::OpenAiCompat,
            Family::OpenAiResponses,
            Family::Anthropic,
            Family::Gemini,
        ] {
            // 工具数上限不再有族级硬默认（bug 24）：只有渠道声明才拦
            assert_eq!(caps_of(f).max_tools, None);
        }
    }

    #[test]
    fn json_schema_native_families_are_supported() {
        let mut ext = Map::new();
        ext.insert("response_format".into(), response_format_json_schema());
        let req = req_with_extensions(ext);

        for family in [Family::OpenAiCompat, Family::OpenAiResponses] {
            let plan = plan_compatibility(&req, caps_of(family));
            let dec = plan.response_format.as_ref().unwrap();
            assert_eq!(dec.action, DecisionAction::Supported, "{family:?}");
            assert!(plan.output_contract.is_none());
        }
    }

    #[test]
    fn json_schema_anthropic_gemini_degrade_to_instruction() {
        let mut ext = Map::new();
        ext.insert("response_format".into(), response_format_json_schema());
        let req = req_with_extensions(ext);

        for family in [Family::Anthropic, Family::Gemini] {
            let mut req = req.clone();
            let plan = plan_compatibility(&req, caps_of(family));
            assert_eq!(
                plan.response_format.as_ref().unwrap().action,
                DecisionAction::Degraded
            );
            let contract = plan.output_contract.as_ref().unwrap();
            assert!(contract.instruction.is_some(), "{family:?}");
            assert!(contract
                .instruction
                .as_ref()
                .unwrap()
                .contains("JSON Schema"));
            assert!(contract.validate_output, "strict 时应做输出校验");

            let outcome = plan.resolve(&mut req);
            assert!(outcome.rejection.is_none());
            assert!(req
                .system
                .iter()
                .any(|s| s.contains("Return only valid JSON")));
            // InstructionOnly：不覆写 response_format，且保留原值（encoder 不读）
            let kept = req.extensions.get("response_format").unwrap();
            assert_eq!(
                kept.get("type").and_then(Value::as_str),
                Some("json_schema")
            );
        }
    }

    #[test]
    fn json_object_degrades_by_instruction_only_for_anthropic() {
        let mut ext = Map::new();
        ext.insert("response_format".into(), json!({"type": "json_object"}));
        let req = req_with_extensions(ext);
        let mut req = req.clone();
        let plan = plan_compatibility(&req, caps_of(Family::Anthropic));
        assert_eq!(
            plan.response_format.as_ref().unwrap().action,
            DecisionAction::Degraded
        );
        let outcome = plan.resolve(&mut req);
        assert!(outcome.rejection.is_none());
        assert!(!outcome.warnings.is_empty());
    }

    #[test]
    fn degraded_instruction_merges_into_existing_system() {
        // anthropic 出站要求 system 单条：指令必须合并进末条而非新增
        let mut ext = Map::new();
        ext.insert("response_format".into(), response_format_json_schema());
        let mut req = req_with_extensions(ext);
        req.system.push("Be concise.".into());
        let plan = plan_compatibility(&req, caps_of(Family::Anthropic));
        let outcome = plan.resolve(&mut req);
        assert!(outcome.rejection.is_none());
        assert_eq!(req.system.len(), 1, "指令应合并进单条 system");
        assert!(req.system[0].starts_with("Be concise."));
        assert!(req.system[0].contains("Return only valid JSON"));
    }

    #[test]
    fn json_object_native_for_openai_compat() {
        let mut ext = Map::new();
        ext.insert("response_format".into(), json!({"type": "json_object"}));
        let req = req_with_extensions(ext);
        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        assert_eq!(
            plan.response_format.as_ref().unwrap().action,
            DecisionAction::Supported
        );
    }

    #[test]
    fn unknown_response_format_rejected() {
        let mut ext = Map::new();
        ext.insert("response_format".into(), json!({"type": "weird"}));
        let req = req_with_extensions(ext);
        let mut req = req.clone();
        let plan = plan_compatibility(&req, caps_of(Family::Anthropic));
        let outcome = plan.resolve(&mut req);
        let (msg, code) = outcome.rejection.unwrap();
        assert_eq!(code, Some("response_format_not_supported"));
        assert!(msg.contains("weird"));
    }

    #[test]
    fn json_object_degrade_to_provider_format_for_chat_compat_target() {
        // anthropic 无 json_object 成员；若未来某族 degraded=JsonObject，应覆写 extensions。
        let caps = Capabilities {
            family: Family::OpenAiCompat,
            parameters: set(&[]),
            tools: set(&["function"]),
            tools_degraded: &[],
            max_tools: None,
            tool_choice: set(&["auto"]),
            response_formats: set(&["text"]),
            degraded_formats: &[("json_schema", FormatDegradation::JsonObject)],
            reasoning: EffortMode::None,
            streaming_usage: true,
            tool_result_images: true,
        };
        let mut ext = Map::new();
        ext.insert("response_format".into(), response_format_json_schema());
        let mut req = req_with_extensions(ext);
        let plan = plan_compatibility(&req, &caps);
        let outcome = plan.resolve(&mut req);
        assert!(outcome.rejection.is_none());
        let format = req.extensions.get("response_format").unwrap();
        assert_eq!(
            format.get("type").and_then(Value::as_str),
            Some("json_object")
        );
        assert!(req.system.iter().any(|s| s.contains("JSON Schema")));
    }

    #[test]
    fn reasoning_effort_decisions() {
        let mut req: CanonicalRequest = Default::default();
        req.params.reasoning_effort = Some("high".into());

        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        assert_eq!(
            plan.reasoning.as_ref().unwrap().action,
            DecisionAction::Supported
        );

        let plan = plan_compatibility(&req, caps_of(Family::Anthropic));
        assert_eq!(
            plan.reasoning.as_ref().unwrap().action,
            DecisionAction::Degraded
        );

        let plan = plan_compatibility(&req, caps_of(Family::Gemini));
        assert_eq!(
            plan.reasoning.as_ref().unwrap().action,
            DecisionAction::Ignored
        );
    }

    /// bug 24 回归：工具数上限只来自**渠道声明**。
    /// - 未声明（族级现在一律 None）：**不拦** —— 140 个工具曾被网关自己的 128 硬默认
    ///   400 掉，而上游实测 140/300 都 200，等于误杀；
    /// - 渠道声明了：超限依旧 400（`tools_limit_exceeded`），错误码与旧行为一致。
    #[test]
    fn max_tools_only_enforced_when_channel_declares_it() {
        let mut req: CanonicalRequest = Default::default();
        for i in 0..140 {
            req.tools.push(crate::codec::ir::ToolSpec {
                name: format!("t{i}"),
                description: None,
                input_schema: json!({"type": "object"}),
            });
        }

        // 1) 未声明 → 放行（上游裁决）
        let mut passthrough_req = req.clone();
        let plan = plan_compatibility(&passthrough_req, caps_of(Family::OpenAiCompat));
        assert!(plan.resolve(&mut passthrough_req).rejection.is_none());

        // 2) 声明 128 → 拦
        let policy = ChannelPolicy {
            effort: None,
            max_tools: Some(128),
            reasoning_replay: false,
        };
        let mut rejected_req = req.clone();
        let plan =
            plan_compatibility_with(&rejected_req, caps_of(Family::OpenAiCompat), Some(&policy));
        let (msg, code) = plan.resolve(&mut rejected_req).rejection.unwrap();
        assert_eq!(code, Some("tools_limit_exceeded"));
        assert!(msg.contains("140"), "文案应含实际个数：{msg}");
        assert!(msg.contains("128"), "文案应含声明上限：{msg}");

        // 3) 声明 300 → 140 个放行
        let wide = ChannelPolicy {
            effort: None,
            max_tools: Some(300),
            reasoning_replay: false,
        };
        let mut ok_req = req.clone();
        let plan = plan_compatibility_with(&ok_req, caps_of(Family::OpenAiCompat), Some(&wide));
        assert!(plan.resolve(&mut ok_req).rejection.is_none());
    }

    #[test]
    fn body_tool_count_reads_tools_array() {
        assert_eq!(body_tool_count(br#"{"tools":[{"a":1},{"b":2}]}"#), Some(2));
        assert_eq!(body_tool_count(br#"{"model":"x"}"#), None);
        assert_eq!(body_tool_count(b"not-json"), None);
    }

    #[test]
    fn tool_choice_specific_undeclared_rejected() {
        let mut req: CanonicalRequest = Default::default();
        req.tools.push(crate::codec::ir::ToolSpec {
            name: "a".into(),
            description: None,
            input_schema: json!({"type": "object"}),
        });
        req.tool_choice = ToolChoice::Specific("b".into());
        let mut req = req.clone();
        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        let outcome = plan.resolve(&mut req);
        let (msg, code) = outcome.rejection.unwrap();
        assert_eq!(code, Some("tool_choice_not_supported"));
        assert!(msg.contains("b"));
    }

    #[test]
    fn tool_choice_specific_declared_supported() {
        let mut req: CanonicalRequest = Default::default();
        req.tools.push(crate::codec::ir::ToolSpec {
            name: "a".into(),
            description: None,
            input_schema: json!({"type": "object"}),
        });
        req.tool_choice = ToolChoice::Specific("a".into());
        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        assert_eq!(
            plan.tool_choice.as_ref().unwrap().action,
            DecisionAction::Supported
        );
    }

    #[test]
    fn n_and_logprobs_rejected() {
        let mut ext = Map::new();
        ext.insert("n".into(), json!(2));
        ext.insert("logprobs".into(), json!(true));
        let req = req_with_extensions(ext);
        let mut req = req.clone();
        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        let outcome = plan.resolve(&mut req);
        let (msg, code) = outcome.rejection.unwrap();
        assert!(code.is_none());
        assert!(msg.contains("n>1"));
    }

    #[test]
    fn n_is_one_is_not_rejected() {
        let mut ext = Map::new();
        ext.insert("n".into(), json!(1));
        let req = req_with_extensions(ext);
        let mut req = req.clone();
        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        let outcome = plan.resolve(&mut req);
        assert!(outcome.rejection.is_none());
    }

    #[test]
    fn unknown_extension_fields_warn_lenient() {
        // 真正未建模的陌生字段 → Lenient WARN
        let mut ext = Map::new();
        ext.insert("future_field_xyz".into(), json!(true));
        let req = req_with_extensions(ext);
        let mut req = req.clone();
        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        let outcome = plan.resolve(&mut req);
        assert!(outcome.rejection.is_none());
        assert!(outcome
            .warnings
            .iter()
            .any(|w| w.contains("future_field_xyz")));
    }

    #[test]
    fn protocol_standard_fields_silent_no_capability_warn() {
        // Responses 协议标准字段（store 等）转换丢弃属预期——不刷 CapabilityWarn
        let mut ext = Map::new();
        ext.insert("store".into(), json!(true));
        ext.insert("parallel_tool_calls".into(), json!(true));
        ext.insert("metadata".into(), json!({"k": "v"}));
        let req = req_with_extensions(ext);
        let mut req = req.clone();
        let plan = plan_compatibility(&req, caps_of(Family::OpenAiCompat));
        let outcome = plan.resolve(&mut req);
        assert!(outcome.rejection.is_none());
        assert!(
            outcome.warnings.is_empty(),
            "标准字段不应产生 CapabilityWarn: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn build_instruction_contains_schema_and_strict_override() {
        let instruction = build_json_instruction(&response_format_json_schema());
        assert!(instruction.contains("Schema name: weather"));
        assert!(instruction.contains("\"temp\""));
        assert!(instruction.contains("Final output format override"));
    }

    #[test]
    fn plain_json_object_instruction_has_no_schema() {
        let instruction = build_json_instruction(&json!({"type": "json_object"}));
        assert!(instruction.contains("Return only valid JSON"));
        assert!(!instruction.contains("JSON Schema"));
    }
}
