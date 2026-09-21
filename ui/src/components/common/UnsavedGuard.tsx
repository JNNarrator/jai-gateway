// 未保存改动的统一拦截 UI（挂在 App 根部，全局唯一实例）。
//
// 两个拦截点：
//   ① 切页：`nav.tsx` 的 setTab → requestLeave() → 本组件弹确认框（受 subscribePrompt 驱动）；
//   ② 关窗/刷新：window 的 `beforeunload` —— 有脏源时 preventDefault + returnValue=""，
//      交给 WebView / 浏览器弹它自己的原生确认（这是浏览器规范要求的最小实现：
//      现代浏览器只认「有没有调过 preventDefault」，自定义文案会被忽略）。
//
// 为什么直接拼 AlertDialog 而不用 common/ConfirmDialog：
// ConfirmDialog 的取消按钮文案是**硬编码的「取消」**，没有 cancelText 这类 prop，
// 而本处的取消语义必须是「留在此页」（用户看到的是「留在页面」而不是「取消一个框」）。
// ConfirmDialog.tsx 不在本次允许改动的文件清单里，所以这里用同一套 alert-dialog
// 原语 + 同样的按钮排布复刻，视觉与 ConfirmDialog 完全一致。
import { useEffect, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { cn } from "@/lib/utils";
import { isAnyDirty, resolveLeave, subscribePrompt } from "@/lib/dirty";

export function UnsavedGuard() {
  // null = 不弹框；数组 = 弹框并点名这些表单
  const [labels, setLabels] = useState<string[] | null>(null);

  useEffect(() => subscribePrompt(setLabels), []);

  // 覆盖关窗 / 刷新（Cmd+R、WebView 关闭按钮触发的卸载都走这条）
  useEffect(() => {
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      if (!isAnyDirty()) return;
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, []);

  return (
    <AlertDialog
      open={labels !== null}
      onOpenChange={(o) => {
        // Esc / 取消按钮都落到这里；confirm 按钮自己的 onClick 先跑（见下方注释）
        if (!o) resolveLeave(false);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>有未保存的改动</AlertDialogTitle>
          <AlertDialogDescription>
            {(labels ?? []).map((l) => `「${l}」有未保存的改动，离开将丢失。`).join("")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          {/* AlertDialogCancel 是 radix 的 DialogClose：点击 → onOpenChange(false) → resolveLeave(false)。
              另外 AlertDialogContent 默认把焦点放在它身上（不放在破坏性按钮上）。 */}
          <AlertDialogCancel>留在此页</AlertDialogCancel>
          {/* 这里必须显式 onClick：radix 的 DialogClose 是
              `composeEventHandlers(props.onClick, () => onOpenChange(false))`，
              即先跑本回调（执行挂起的离开动作）、再关框；反过来写会先关框清掉待执行动作，
              「放弃改动并离开」就会变成静默无操作。 */}
          <AlertDialogAction
            className={cn(
              "bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:bg-destructive/60 dark:text-white",
            )}
            onClick={() => resolveLeave(true)}
          >
            放弃改动并离开
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
