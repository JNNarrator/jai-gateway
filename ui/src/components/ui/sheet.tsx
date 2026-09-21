import * as React from "react"
import { XIcon } from "lucide-react"
import { Dialog as DialogPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

/**
 * 右侧详情抽屉（sheet）—— 基于 radix Dialog 的**浮动面板**，不是满高侧栏。
 *
 * 为什么不做满高（`inset-y-0`）：门禁 gate.mjs 有一条判据「任何 role=dialog 的
 * footer 按钮矩形不得与右下角 toast 占位带相交」。toast 固定在右下
 * （宽 356 / 高 54 / 距视口边 24），占位带 = x∈[vw-380, vw-24]、y∈[vh-78, vh-24]。
 * 满高抽屉的 footer 必然落进这条带（footer 贴着视口底边）→ 判失败。
 * 取 `bottom-24`（底部留 96px）后，footer 按钮底边 = vh-96-24(p-6) = vh-120，
 * 恒在 toast 带上沿（vh-78）之上 **42px**，与视口尺寸无关。
 *
 * 三段式与 dialog.tsx 完全同构（Header 常驻 / Body 唯一滚动层 / Footer 常驻），
 * 并沿用同一套 `data-slot` 命名（`dialog-content` / `dialog-header` / `dialog-body`
 * / `dialog-footer` / `dialog-close`），使既有探针（audit.mjs 扫 `[role=dialog]`、
 * fold.mjs 扫 `[data-slot=dialog-content]`、probe-hits.mjs 扫 `[role=dialog]`）
 * 无需改动即可覆盖抽屉。
 */

function Sheet({ ...props }: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="sheet" {...props} />
}

function SheetTrigger({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="sheet-trigger" {...props} />
}

function SheetClose({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function SheetOverlay({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-0 z-50 bg-black/50 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0",
        className
      )}
      {...props}
    />
  )
}

function SheetContent({
  className,
  children,
  showCloseButton = true,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
}) {
  return (
    <DialogPrimitive.Portal data-slot="sheet-portal">
      {/* 遮罩：点击即关（radix 默认行为），Esc 同样可关 */}
      <SheetOverlay />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          // 不贴底浮动面板：top-4 / right-4 / bottom-24（见文件头注释）
          "fixed top-4 right-4 bottom-24 z-50 flex w-[420px] max-w-[calc(100vw-2rem)] flex-col gap-4 overflow-hidden rounded-lg border bg-background p-6 shadow-lg duration-200 outline-none data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-right data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:slide-in-from-right",
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close
            data-slot="dialog-close"
            aria-label="关闭"
            className="absolute top-3 right-3 grid size-6 place-items-center rounded-sm opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:outline-hidden disabled:pointer-events-none"
          >
            <XIcon className="size-4" aria-hidden />
            <span className="sr-only">关闭</span>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  )
}

function SheetHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex shrink-0 flex-col gap-2 pr-6 text-left", className)}
      {...props}
    />
  )
}

/** 抽屉正文容器：整块限高（见 SheetContent），头尾常驻，只有这一层滚动。 */
function SheetBody({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-body"
      className={cn("-mx-6 min-h-0 flex-1 overflow-y-auto px-6", className)}
      {...props}
    />
  )
}

function SheetFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "-mx-6 flex shrink-0 flex-col-reverse gap-2 border-t px-6 pt-4 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    />
  )
}

function SheetTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("text-base leading-tight font-semibold", className)}
      {...props}
    />
  )
}

function SheetDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn("text-xs text-muted-foreground", className)}
      {...props}
    />
  )
}

export {
  Sheet,
  SheetBody,
  SheetClose,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetOverlay,
  SheetTitle,
  SheetTrigger,
}
