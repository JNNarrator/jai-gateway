//! gateway-core — JAI 网关核心库
//!
//! 模块布局（对应 docs/design/* 三份设计文档）：
//! - `codec`  : 协议中间表示与各协议适配器（M4 起填充；现含 OpenAI 旁路工具）
//! - `router` : 多渠道路由与故障转移执行器（M2 起填充）
//! - `store`  : SQLite 唯一事实源 + 迁移执行器 + 异步日志管道
//! - `server` : Axum 网关 —— 绑定/端口顺延、安全中间件、直通代理、healthz
//! - `vault`  : OS 密钥环封装（供应商凭据生命周期）
//! - `discover`: 上游模型发现（三家协议适配）

pub mod codec;
pub mod discover;
pub mod mcp;
pub mod router;
pub mod server;
pub mod skills;
pub mod store;
pub mod sync;
pub mod vault;
