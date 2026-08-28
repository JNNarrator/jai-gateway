# Bug / 优化清单

> 本文件是 JAI 唯一问题追踪入口。以后所有 bug 和优化都放这里。
> 完成一项就把 `[ ]` 改成 `[x]`，并尽量补一行“解决版本 / 说明 / 日期”。

## 1. Bug 清单

- [x] 1. 创建供应商报错：系统密钥环操作失败（检查本机凭据设置）: keyring: Platform secure storage failure: UNIX[Operation not permitted]
  - 已解决：密钥环不可用时自动降级为数据目录下 `vault_fallback.json`（Unix 0600），设置页可见“文件降级”状态；真实系统仍优先钥匙串。

- [x] 2. dsh 测试返回 Cloudflare 400 Bad Request
  - 现象：
    ```
    OpenAI API error (400): {"code":null,"message":"<html>... <title>400 Bad Request</title> ... cloudflare ...","type":"invalid_request_error"}
    ```
  - 根因：dsh 走 OpenAI Responses API（`/v1/responses`），one-model 也是 Responses 上游；但 JAI 把该渠道按 `openai_compat` 转成 `/chat/completions`，被上游 Cloudflare 拒 400。
  - 已解决：
    1. 新增 `openai_responses` 协议族 + 迁移 0003，Responses 入站遇到该协议族直接字节透传到 `/responses`；
    2. 顺带修复直通转发丢失 `Content-Type`/`Accept`/`User-Agent` 等头的问题；
    3. ✅ 已用 `dsh --profile headless "直接回复：JAI链路正常"` 实测通过，返回 `JAI链路正常`，退出码 0。

- [x] 3. dsh 多供应商后 400（ALB / 模型不可用）
  - 现象：
    ```
    OpenAI API error (400): <html>...<center>alb</center>...
    OpenAI API error (400): {"code":"MODEL_NOT_AVAILABLE","message":"模型不可用：基元律动/glm-5",...}
    ```
  - 根因：
    1. 跨族转换路径给上游发了两个 `Authorization` 头（一个空占位、一个真实 Key），被 ALB 拒 400；
    2. 限定模型名 `供应商/模型` 被原样转发给上游，上游不认 `基元律动/glm-5`。
  - 已解决：
    1. 转换路径去掉空占位鉴权头，只发一个真实 Authorization；
    2. 路由前把 `供应商/模型` 重写为真实模型名 `glm-5` 再转发；
    3. Responses 入站优先同协议族直通，避免被跨族渠道截胡。
  - ✅ 已用 `dsh --profile headless` 实测退出码 0。

## 2. 优化清单

- [x] 1. 创建供应商弹框应该有按钮可以测试能不能获取到模型。
- [x] 2. skill 添加应该支持 zip 导入添加。
- [x] 3. UI/UX 优化：按 `docs/ui优化.md` 的 66 项建议逐项实施并勾选。
- [x] 4. 多供应商时，dsh 模型列表显示“供应商名/模型名”，请求支持按该限定 ID 路由。