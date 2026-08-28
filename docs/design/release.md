# 发布工程（M9）检查单

> 状态：开发侧已就绪；实际签名/公证/更新通道需在 CI secrets 与发布主机上完成。

## 1. 版本与产物

- 版本：`src-tauri/tauri.conf.json` `"version"`
- 产物：
  - macOS：`.dmg` / `.app`（Tauri bundle `targets: all`）
  - Windows：`.msi` / `.exe`（NSIS 或 MSI）
- 更新通道：Tauri Updater
  - 公钥签名：`tauri signer generate`
  - `tauri.conf.json` 增加 `plugins.updater.pubkey` 与 `endpoints`
  - CI 使用 `tauri-apps/tauri-action` 上传产物并生成 `latest.json` feed

## 2. 签名与公证

- macOS：
  - 环境变量：`APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`
  - CI：`tauri-apps/tauri-action` 自动完成签名与 notarization（需 Developer ID 证书）
- Windows：
  - 环境变量：`WINDOWS_CERTIFICATE`（PFX base64）、`WINDOWS_CERTIFICATE_PASSWORD`
  - 杀软误报排查流程见仓库文档

## 3. 发布前门禁

- [x] 自动化门禁脚本：`bash scripts/release_check.sh`（工作区干净、版本号、CHANGELOG、tag、全量回归）
- [ ] `bash scripts/regression.sh` 全绿（已被 release_check.sh 覆盖）
- [ ] 黄金夹具矩阵：M2/M3/M4/M5/M6/M7/M8 集成测试全绿
- [ ] 真机验收：Claude Code、Codex、DeepSeek harness、zcode 各至少一例
- [ ] 48h 本机常驻观察零崩溃
- [ ] 签名/公证在干净 VM 验证安装包
- [ ] 更新通道从上一版升级成功
- [ ] README 接入指南与 CHANGELOG 终稿

## 4. 发布流程

1. 创建 `release/v0.1.0-beta` 分支并跑全量回归
2. 更新 `CHANGELOG.md`、`README.md`、版本号
3. 推送 tag `v0.1.0-beta`
4. 触发 `.github/workflows/release.yml`
5. 人工验收 CI 产物（macOS + Windows）
6. 在 GitHub Releases 发布并指向 updater feed