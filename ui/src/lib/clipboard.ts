import { toast } from "./toast";

export function copyText(text: string) {
  navigator.clipboard?.writeText(text).then(
    () => toast("已复制"),
    () => toast("复制失败", "err")
  );
}
