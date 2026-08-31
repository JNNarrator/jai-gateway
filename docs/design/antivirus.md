# 杀软误报排查流程

> 状态：建档（M9 验收项 4）。首次实际遇到误报后再补充厂商申诉记录。

## 背景

Tauri 2 产物（`.dmg` / `.exe`）因代码签名缺失或新签名证书信任未建立，可能被
Windows SmartScreen / Defender 或 macOS Gatekeeper 拦截。未签名产物**不可用于分发**，
分发前必须完成代码签名（见 [release.md](release.md) §2）。

## 排查步骤

### 1. 复现与定位

1. 确认用户安装的是**已签名**产物（未签名产物被拦截属预期行为，先完成签名）。
2. Windows 查事件日志：
   - 打开 `Windows 安全中心 → 保护历史记录`
   - 记录被拦截文件路径、检测名称（Detection name）与威胁等级
3. macOS 查 Gatekeeper：
   - `spctl -a -vv /path/to/JAI.app` 查看评估结果
   - 若提示 "unidentified developer"，先核验签名：`codesign --verify --deep --strict`

### 2. 先自查（排除自身问题）

- [ ] 签名有效：`codesign --verify` / Windows 右键属性→数字签名 无警告
- [ ] 产物从官方渠道发布（GitHub Releases 草稿转正式）
- [ ] 不携带被误报的常见特征：无自解压脚本、无异常网络回连、无提权行为
- [ ] 更新通道 feed 与公钥正确（见 release.md §1）

### 3. 申诉渠道

- Windows Defender：<https://www.microsoft.com/en-us/wdsi/filesubmission>
  - 提交文件 + 签名信息，说明 JAI 是本地 AI API 网关（开源，MIT）
- SmartScreen：<https://www.microsoft.com/en-us/windows/false-positive>
- macOS Gatekeeper：`xattr -d com.apple.quarantine` 仅用于本地开发机，不解决分发问题；
  正式通道走 Developer ID 签名 + 公证

### 4. 记录

每次实际误报处理，在此追加：日期、检测名、厂商、提交编号、结果、对构建配置的改动。

---

> 关联：构建配置见 [release.md](release.md)；签名 secrets 见仓库 CI 设置页。
