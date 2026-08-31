## Task 7: 阶段 0 终验与推送

**Files:**
- 无代码改动（只验证 + 推送）

**Interfaces:**
- Consumes: Task 1–6 全部产出

- [ ] **Step 1: 构建硬门槛**

```bash
pnpm --dir ui build
```

Expected: `tsc --noEmit` + `vite build` 零错误。贴出完整输出末尾的构建成功信息。

- [ ] **Step 2: 手工验收清单**

`pnpm --dir ui dev` 后逐项过：

| # | 检查项 | 预期 |
| --- | --- | --- |
| 1 | 9 个页签切换 | 页面正常渲染，无空白/报错 |
| 2 | 亮/暗/跟随系统切换 | 全页颜色跟随，持久化 |
| 3 | 侧边栏折叠/展开 | 动画顺畅，状态持久 |
| 4 | toast（成功/失败） | sonner 样式，底部居中 |
| 5 | 跨页跳转（同步→网关） | goTab 仍工作 |
| 6 | 复制按钮 | 成功 toast |
| 7 | 危险操作按钮（轮换/删除） | 与原行为一致（未迁移，仍原样） |
| 8 | 网关状态/供应商列表等数据加载 | 与 Task 1 拆分前一致（接口行为未动） |

- [ ] **Step 3: 检查 git 干净度**

```bash
git status --short
```

Expected: 除任务提交外，仅剩无关的既有脏文件（如 `crates/gateway-core/src/server/proxy.rs` 等），不得有 `ui/` 下的意外改动。

- [ ] **Step 4: 推送**

```bash
git push origin main
```

- [ ] **Step 5: 汇报阶段 0 完成**

按 AGENTS.md 格式输出 `=== 完成 ===`，验证指标 = `pnpm --dir ui build` 零错误 + 验收清单逐项通过。

---

## Self-Review（已执行）

1. **Spec 覆盖**：spec 阶段 0 各项（Tailwind 3→4 ✓ Task 2；shadcn 初始化+基础件 ✓ Task 3；双主题变量与切换器 ✓ Task 3；sonner 替换 toast 签名不变 ✓ Task 4；App.tsx 机械拆分 ✓ Task 1；侧边栏导航+折叠持久化 ✓ Task 6；goTab CustomEvent 换 Context ✓ Task 5；每阶段构建零错误 ✓ 每 Task 验证步 + Task 7）。
2. **占位符扫描**：无 TBD/TODO/「适当处理」类表述；所有代码步骤给出完整代码或精确命令。
3. **类型一致性**：`toast(msg, kind)` 签名 Task 1 定义 → Task 4 保持；`goTab(tab)` Task 1 定义 → Task 5 保持（22 处 toast、2 处 goTab 调用点零改动）；`Tab` union Task 1/5 一致；`cn()` 仅 Task 3 定义、Task 6 使用；`useNav` 仅 Task 5 定义、Task 5/6 使用；`ThemeToggle` Task 3 定义、Task 3 临时挂载 + Task 6 移入侧边栏。
4. **已知过渡期瑕疵**（spec 风险表第 1 条落点）：红/绿状态 banner 亮色对比度偏低，已显式登记在 Task 3 Step 10 冒烟清单，由阶段 2–5 逐页迁移治理；兼容映射块在 Task 3 Step 6 CSS 中以注释标注「阶段 2–5 删除」。
