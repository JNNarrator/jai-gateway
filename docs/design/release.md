# 发布工程（M9）检查单

> 状态：开发侧已就绪；release.yml 工作流与 updater 签名密钥已生成，实际签名/公证需在 CI secrets 与发布主机上配置证书后执行。

## 1. 版本与产物

- 版本：`src-tauri/tauri.conf.json` `"version"`（当前 `0.4.1`）
- 产物：
  - macOS：`.dmg` / `.app`（Tauri bundle `targets: all`）
    - ⚠️ **只有 `aarch64`（Apple Silicon）**：`macos-latest` runner 已是 arm64，
      产物为 `JAI_<ver>_aarch64.dmg` + `JAI_aarch64.app.tar.gz(.sig)`，
      updater feed 里也只有 `darwin-aarch64*`。**Intel Mac 既没有安装包、也无法自动更新。**
      自 v0.2.10 起稳定如此（v0.2.10 / v0.2.11 / v0.2.12 三版产物形状完全一致），
      非某次改动的回归。**补 Intel 不是「加一行矩阵」那么简单**，见下方两条路与代价：
      - 路 A（便宜，推荐先试）：在**现有 arm64 runner 上交叉编译** ——
        `rustup target add x86_64-apple-darwin` + `args: --target x86_64-apple-darwin`。
        同 job 内多出一个 bundle，feed 多一个 `darwin-x64` 键，不额外占 runner。
      - 路 B（贵，且要先手工配置）：用 GitHub 的 **macOS x64 larger runner**
        （`macos-15-intel` / `macos-26-intel`）。注意 `macos-13` **已下架**；
        且 `-intel` / `-large` 后缀属 **larger runners：按分钟计费（公开仓库也不免费）**，
        还必须先在 org/repo 设置里创建该 runner，否则 `runs-on` 直接找不到匹配 runner。
        → 不要照抄旧文档里的 `macos-13`，那个 label 已不存在。
  - Windows：`.msi` / `.exe`（NSIS 或 MSI），x64，含 `.sig`
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
- [x] tag 触发验证：**v0.4.0 实测**（run 35839374439，3/3 job success：Create release draft 5s、
      Build windows-latest 9m57s、Build macos-latest 11m33s，整轮 11m51s）——
      产物：`JAI_0.4.0_aarch64.dmg`、`JAI_aarch64.app.tar.gz(+.sig)`、`JAI_0.4.0_x64-setup.exe(+.sig)`、
      `JAI_0.4.0_x64_en-US.msi(+.sig)`、`latest.json`；`prerelease=false`。
      同轮 `CI`（main push，run 35839349074）**三个 job 全绿**（Frontend build 22s /
      Rust windows-latest 6m41s / Rust macos-latest 4m9s，整轮 6m46s）—— v0.3.1 那次
      windows-latest 的日志落库时序抖动未复现（有界轮询的修复生效）。
      **发布**：`gh release edit v0.4.0 --draft=false --latest`（`published=2026-09-23T09:18:53Z`），
      随后 feed 校验通过：`releases/latest/download/latest.json` → `version=0.4.0`，
      platforms = darwin-aarch64 / darwin-aarch64-app / windows-x86_64 / windows-x86_64-msi /
      windows-x86_64-nsis（5 键齐全）。草稿期 feed 仍是上一版（与 §5 记录一致，每次发布都必须跑这条校验）。
- [x] tag 触发验证：**v0.3.1 实测**（run 35711981353，3/3 job success：Create release draft 5s、
      Build macos-latest 7m19s、Build windows-latest 12m23s，整轮 12m35s）——
      产物：`JAI_0.3.1_aarch64.dmg`、`JAI_aarch64.app.tar.gz(+.sig)`、`JAI_0.3.1_x64-setup.exe(+.sig)`、
      `JAI_0.3.1_x64_en-US.msi(+.sig)`、`latest.json`；`prerelease=false`。
      **同轮 `CI`（main push）在 windows-latest 上红过一次，但那是与本次改动无关的测试时序抖动**
      （`m3_anthropic.rs:244` 用固定 `sleep(700ms)` 等异步日志落库；同一提交重跑即绿，
      macOS job 同轮也绿）——已按 bug 清单 15 改成有界轮询，修复落在 tag 之后的提交上。
      **发布**：`gh release edit v0.3.1 --draft=false --latest`（`published=2026-09-22T10:08:15Z`），
      随后 feed 校验通过：`releases/latest/download/latest.json` → `version=0.3.1`，
      platforms = darwin-aarch64 / darwin-aarch64-app / windows-x86_64 / windows-x86_64-msi /
      windows-x86_64-nsis（5 键齐全）。发布前实测 feed 仍是 `version=0.3.0`（草稿期不生效，
      与 §5 记录一致）。
- [x] tag 触发验证：**v0.2.13 实测**（run 35585047318，3/3 job success：Create draft 6s、Build windows-latest 12m37s、Build macos-latest 7m22s）——
      产物：`JAI_0.2.13_aarch64.dmg`、`JAI_aarch64.app.tar.gz(+.sig)`、`JAI_0.2.13_x64-setup.exe(+.sig)`、`JAI_0.2.13_x64_en-US.msi(+.sig)`、`latest.json`；`prerelease=false`。
      **草稿必须人工发布**：`draft=true` 期间 `releases/latest/download/latest.json` 仍指向上一版（实测 `version=0.2.12`，**无任何报错**），执行 `gh release edit v0.2.13 --draft=false` 后 feed 才更新为 `version=0.2.13`（`published=2026-09-21T10:26:52Z`，platforms = darwin-aarch64 / darwin-aarch64-app / windows-x86_64 / windows-x86_64-msi / windows-x86_64-nsis）。
      **「草稿未发布」与「草稿建成 prerelease」是同一类翻车点（v0.2.6），每次发布都必须跑 feed 校验。**
- [x] tag 触发验证：**v0.2.12 实测**（run 35552099194，12m34s，3/3 job success）——
      macOS dmg + app.tar.gz/.sig、Windows msi/exe + .sig、`latest.json` 齐全；
      `prerelease=false`；发布后 feed 校验 `version=0.2.12`
      （`https://github.com/JNNarrator/jai-gateway/releases/latest/download/latest.json`）。
      该步是 v0.2.6 的翻车点（草稿建成 prerelease 会让 feed 停在上一版且无报错），
      每次发布都必须跑。

## 4. 发布前门禁

- [x] 自动化门禁脚本：`bash scripts/release_check.sh`（工作区干净、版本号、CHANGELOG、tag、全量回归）
- [x] `bash scripts/regression.sh` 全绿（已被 release_check.sh 覆盖）
- [ ] **D9 批次（v0.4.0）的真机验收** —— 自动化门禁覆盖不到、必须手点的三项：
      - 单实例保护（D9-T4）：连点两次图标只出一个窗口、第二次把已有窗口前置
      - 端点探测（D9-T1）：在「新建供应商」里填**真实上游**点一次探测，逐端点结论与延迟合理
      - 多密钥 + 白/黑名单（D9-T6a/T6b）：给一把密钥配上规则，在真实客户端里跑一次
        「能用的模型能用 / 被限制的模型返回 403 且文案点名规则」
- [ ] 黄金夹具矩阵：M2/M3/M4/M5/M6/M7/M8 集成测试全绿
- [ ] 真机验收：Claude Code、Codex、DeepSeek harness、zcode 各至少一例
- [x] **WebDAV 真机验收（v0.3.1 实测通过，2026-09-22）**：一条命令跑完整链路
      （`crates/gateway-core/tests/webdav_live_e2e.rs`，`#[ignore]`，不随常规回归跑）：
      ```bash
      JAI_DAV_LIVE_URL=https://dav.example.com JAI_DAV_LIVE_USER=user JAI_DAV_LIVE_PASS=pass \
      JAI_DAV_LIVE_DIR=jai-e2e-$(date +%s) \
        cargo test -p gateway-core --test webdav_live_e2e -- --ignored --nocapture
      ```
      **`JAI_DAV_LIVE_DIR` 必须是隔离目录**：用例只在该目录内增删，结束时会清空并尝试删掉它。
      覆盖：连接测试 → 推送 → 拉取逐字节比对 → 覆盖前留存时间戳备份 → 备份列表/读取/删除
      （含幂等删除）→ 第二台机器 `apply_import` 落库（供应商 / 上游密钥 / 模型 / 网关 Key）
      → 空目录的 404 语义 → 自动清理。实测输出见 CHANGELOG 的 v0.3.1 条目。
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
7. **发布后必须校验 updater 通道**（v0.2.6 踩过：草稿转正式后 feed 仍返回上一版）

   ```bash
   curl -sL https://github.com/<owner>/<repo>/releases/latest/download/latest.json | jq .version
   # 必须等于刚发布的版本号；否则：
   gh release edit vX.Y.Z --prerelease=false --latest
   ```

   原因：updater 端点走 `/releases/latest/…`，而 **GitHub 的 latest 不含 prerelease**。
   `release.yml` 早期把草稿建成 `prerelease=true`（v0.2.6 已改为 `false`），
   若沿用到旧工作流，发布后更新通道会一直停在上一版且**没有任何报错**。

## 6. 本地打 macOS 包（验证用，非发布产物）

CI 与本地的前端钩子 cwd **不一致**，直接用仓库里的 `tauri.conf.json` 本地打包会失败：

- **CI**（`tauri-apps/tauri-action`，未设 `projectPath`）：以 `src-tauri` 为 cwd 执行
  `beforeBuildCommand`，故配置里的 `pnpm --dir ../ui build` 成立（v0.1.9 Release 运行日志可证）。
- **本地** `cargo tauri build`（tauri-cli 2.11.4）：前端钩子 cwd = **仓库根**，`../ui` 会解析到仓库外
  → `ERR_PNPM_ENOENT … lstat '<上级目录>/ui'`。**不要为此改仓库配置**（会弄坏 CI）。

本地正确做法——用覆盖配置把钩子换掉（UI 产物路径不受影响，`frontendDist` 仍相对 `src-tauri` 解析）：

```bash
cd src-tauri
cat > /tmp/jai-local-hook.json <<'EOF'
{"build":{"beforeBuildCommand":"pnpm --dir ui build"}}
EOF
TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/jai.key)" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat ~/.tauri/jai.key.password)" \
cargo tauri build --bundles app --config /tmp/jai-local-hook.json
# 产物：target/release/bundle/macos/JAI.app（+ updater 用的 .app.tar.gz / .sig）
```

注意：本地包是 **ad-hoc 签名**（与 CI 未配 Apple 证书时的产物同级），仅供本机自测/自己更新，
**不要**当作分发给用户的正式产物；正式产物一律由 tag 触发的 CI 生成，并据此更新 Release / updater feed。
