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
  - 已解决：直通转发原来只带鉴权头和 extra_headers，丢失了下游的 `Content-Type`、`Accept`、`User-Agent` 等头，被 Cloudflare 侧 400 拒绝。现已把安全请求头透传给上游（排除 Authorization / x-api-key，避免泄漏网关 Key）；**待真机 dsh 复测确认**。

## 2. 优化清单

- [x] 1. 创建供应商弹框应该有按钮可以测试能不能获取到模型。
- [x] 2. skill 添加应该支持 zip 导入添加。