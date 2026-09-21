// 模块级「脏状态注册表」：表单声明自己「有未保存的改动」，由**统一入口**拦截离开。
//
// 存在意义：此前全仓库没有任何脏状态追踪（无 beforeunload、无 dirty 标记），
// 用户改了表单不保存就切页/关窗 → 静默丢失。把追踪放在模块级而不是 React context，
// 是因为两个拦截点都拿不到组件树：
//   ① 切页发生在 `nav.tsx` 的 `setTab`（可能由侧栏、命令面板、页面内 goTab 触发）；
//   ② 关窗/刷新发生在 window 的 `beforeunload`。
// 模块级单例让「谁脏」与「谁拦」解耦：表单只管注册，拦截点只管问 `isAnyDirty()`。
//
// 职责划分：
//   useDirtyGuard  —— 表单侧：挂载注册 / 卸载注销 / dirty 变化时更新
//   requestLeave   —— 拦截侧：有脏源则挂起动作 + 通知订阅者弹确认框；无脏源直接执行
//   resolveLeave   —— 确认框侧：关提示；leave === true 才执行被挂起的动作
import { useEffect, useId } from "react";

/** 已注册的脏源：key 是组件实例的稳定 id（useId），value 是给用户看的表单名 */
const dirtySources = new Map<string, string>();

/** 被挂起的「离开动作」：用户点了「放弃改动并离开」才真正执行 */
let pendingLeave: (() => void) | null = null;

/** 确认框订阅者：收到 labels 表示「该弹框了」，收到 null 表示「关掉提示」 */
const promptSubs = new Set<(labels: string[] | null) => void>();

function notify(labels: string[] | null) {
  // 复制一份再遍历：订阅者在回调里退订（如组件卸载）不应影响本轮其它订阅者
  for (const cb of [...promptSubs]) {
    try {
      cb(labels);
    } catch {
      /* 单个订阅者抛错不该拖垮其它订阅者（也不该阻断离开动作） */
    }
  }
}

/** 当前是否存在未保存的改动（beforeunload 拦截用） */
export function isAnyDirty(): boolean {
  return dirtySources.size > 0;
}

/** 当前脏源的表单名（去重，顺序按注册先后）——用于在确认框里点名是哪些表单 */
export function dirtyLabels(): string[] {
  return Array.from(new Set(dirtySources.values()));
}

/**
 * 声明「本组件的表单有未保存的改动」。
 * 组件挂载时注册、卸载时注销、dirty 变化时更新；`dirty === false` 时不占坑。
 */
export function useDirtyGuard(label: string, dirty: boolean): void {
  const id = useId();
  useEffect(() => {
    if (!dirty) {
      dirtySources.delete(id);
      return;
    }
    dirtySources.set(id, label);
    return () => {
      dirtySources.delete(id);
    };
  }, [id, label, dirty]);
}

/**
 * 请求离开（切页 / 页面内跳转）。
 * - 无脏源：直接执行 `onLeave()`（行为与改造前完全一致）；
 * - 有脏源：把 `onLeave` 挂起，通知订阅者弹确认框，等 `resolveLeave` 裁决。
 */
export function requestLeave(onLeave: () => void): void {
  const labels = dirtyLabels();
  if (labels.length === 0) {
    onLeave();
    return;
  }
  pendingLeave = onLeave;
  notify(labels);
}

/** 订阅确认框状态：`labels === null` 表示关掉提示。返回退订函数。 */
export function subscribePrompt(cb: (labels: string[] | null) => void): () => void {
  promptSubs.add(cb);
  return () => {
    promptSubs.delete(cb);
  };
}

/** 裁决：关闭提示；`leave === true` 时执行被挂起的离开动作。 */
export function resolveLeave(leave: boolean): void {
  const action = pendingLeave;
  pendingLeave = null;
  notify(null);
  if (leave && action) action();
}
