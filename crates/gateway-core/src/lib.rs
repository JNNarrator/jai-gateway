//! gateway-core — JAI 网关核心库
//!
//! 模块布局（对应 docs/design/* 三份设计文档）：
//! - `codec`  : 协议中间表示与各协议适配器（M4 起填充）
//! - `router` : 多渠道路由与故障转移执行器（M2 起填充）
//! - `store`  : SQLite 唯一事实源 + 迁移执行器（M0 即落地）
//! - `server` : Axum 网关监督面 —— 绑定、端口顺延、/healthz

pub mod codec;
pub mod router;
pub mod server;
pub mod store;
