# 发布工程（M9）检查单

> 状态：开发侧已就绪；release.yml 工作流与 updater 签名密钥已生成，实际签名/公证需在 CI secrets 与发布主机上配置证书后执行。

## 1. 版本与产物

- 版本：`src-tauri/tauri.conf.json` `"version"`（当前 `0.1.0`）
- 产物：
  - macOS：`.dmg` / `.app`（Tauri bundle `targets: all`）
  - Windows：`.msi` / `.exe`（NSIS 或 MSI）
- 更新通道：Tauri Updater ✅ 已装配
  - 公钥签名：`tauri signer generate` ✅ 已生成（私钥 `~/.tauri/jai.key` + 密码 `~/.tauri/jai.key.password`，**仅存发布主机，勿入库**）
  - `tauri.conf.json` 已配置 `plugins.updater.pubkey` 与 `endpoints`（指向 GitHub Releases `latest.json`）
  - `src-tauri/Cargo.toml` 已启用 `tauri-plugin-updater`，`main.rs` 已注册插件
  - CI 使用 `tauri-apps/tauri-action` 上传产物并生成 `latest.json` feed

## 2. 签名与公证

- macOS：
  - 环境变量：`APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`
  - CI：`tauri-apps/tauri-action` 自动完成签名与 notarization（需 Developer ID 证书）
  - 未配置时产出**未签名**包，可本地自测但不可分发
- Windows：
  - 环境变量：`WINDOWS_CERTIFICATE`（PFX base64）、`WINDOWS_CERTIFICATE_PASSWORD`
  - 杀软误报排查流程：见 [antivirus.md](antivirus.md)（待建档：SmartScreen 规避、误报申诉渠道记录）

## 3. CI 工作流

- [x] `.github/workflows/release.yml`：tag `v*` 触发，macOS + Windows 矩阵构建，
      tauri-action 创建 release 草稿并上传产物，Updater 签名 secrets 缺失即构建失败（强制正确配置）
- [x] Updater 签名密钥已生成并写入本地（见 §1）；CI secrets 需配置：
  - `TAURI_SIGNING_PRIVATE_KEY`：私钥文件内容（`~/.tauri/jai.key`）
  - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：私钥密码（`~/.tauri/jai.key.password`）
- [ ] 首次 tag 触发验证：构建产物（macOS dmg/app、Windows msi/exe）均含 `.sig` 与 updater 元数据

## 4. 发布前门禁

- [x] 自动化门禁脚本：`bash scripts/release_check.sh`（工作区干净、版本号、CHANGELOG、tag、全量回归）
- [x] `bash scripts/regression.sh` 全绿（已被 release_check.sh 覆盖）
- [ ] 黄金夹具矩阵：M2/M3/M4/M5/M6/M7/M8 集成测试全绿
- [ ] 真机验收：Claude Code、Codex、DeepSeek harness、zcode 各至少一例
- [ ] 48h 本机常驻观察零崩溃
- [ ] 签名/公证在干净 VM 验证安装包
- [ ] 更新通道从上一版升级成功
- [ ] README 接入指南与 CHANGELOG 终稿

## 5. 发布流程

1. 创建 `release/v0.1.0-beta` 分支并跑全量回归（`bash scripts/release_check.sh`）
2. 更新 `CHANGELOG.md`、`README.md`、版本号
3. 推送 tag `v0.1.0-beta`
4. 触发 `.github/workflows/release.yml`
5. 人工验收 CI 产物（macOS + Windows，签名 + updater 元数据）
6. 在 GitHub Releases 将草稿转正式发布并指向 updater feed
