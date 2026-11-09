// 供应商 sheet 的返回键与焦点循环逻辑。ArkTS 侧通过 __lcNative.setSheetOpen
// 得知开合状态，再在系统返回键时调用 window.__lcOhosCloseSheet。

/** sheet 内可聚焦元素；与 ProvidersPanel 的实际控件保持一致。 */
export const SHEET_FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

/**
 * 在 [0, length) 内移动一格。焦点不在 sheet 内（-1）或越界时，
 * 正向从头开始、反向从尾开始。
 */
export function nextFocusIndex(
  current: number,
  length: number,
  backwards = false,
): number {
  if (length <= 0) return -1
  if (current < 0 || current >= length) {
    return backwards ? length - 1 : 0
  }
  return backwards
    ? (current - 1 + length) % length
    : (current + 1) % length
}

/** 返回键只在 sheet 打开时被消费；关闭状态下交还默认返回（退出/路由）。 */
export function consumeBackForSheet(open: boolean, close: () => void): boolean {
  if (!open) return false
  close()
  return true
}
