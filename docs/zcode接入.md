# zcode 接入 JAI 指南

> 目标：zcode 通过 JAI 访问上游模型时，体验与直连一致。

## 原理

zcode 支持自定义 Provider，当前本机 zcode 配置使用 `kind: "anthropic"` + `baseURL` 的形态。
因此让 zcode 走 JAI 的最简方式：

- zcode Provider → `http://127.0.0.1:<JAI端口>`（Anthropic 协议）
- API Key → JAI 网关 Key（`sk-jai-*`）
- 模型 → JAI 中已启用且对应该 Provider 的模型名

JAI 收到 Anthropic `/v1/messages` 后会按模型路由到真实上游（OpenAI 兼容 / Anthropic / Gemini / Responses 均可），
跨协议转换由 JAI 完成。

## 在 zcode 中添加 JAI Provider

1. 打开 zcode 的模型/Provider 设置，添加自定义 Provider：
   - 名称：`JAI`
   - 类型/协议：`anthropic`
   - Base URL：`http://127.0.0.1:1314`
   - API Key：JAI 网关页显示的 `sk-jai-xxxx`
2. 添加要使用的模型，模型名与 JAI「模型」页一致。
3. 选择该 Provider 与模型后开始对话。

## 让 zcode 加载 JAI 托管的 MCP

JAI「MCP」页点击 **复制客户端配置**，会得到标准 `mcpServers` JSON。
zcode / Claude Code / Continue 等支持标准 MCP 配置的 Agent 可将其写入自己的 MCP 配置文件，
加载后由 Agent 自行决定何时调用这些 MCP 工具。

## 让 zcode 看到 JAI 托管的 Skill

JAI「技能」页点击 **复制技能包**，会把启用中的技能导出为 Markdown。
可将该文本放入 zcode 的 System Prompt / 项目说明 / 技能目录，Agent 即可看到这些技能内容并自行调度。

## 备注

- 当前 zcode 真机自动回归尚未跑通（GUI 客户端需要人工操作），协议线已按本机 zcode 配置确认走 Anthropic。
- 如 zcode 后续版本支持 OpenAI Responses 或其他协议线，接入方式同步调整。
