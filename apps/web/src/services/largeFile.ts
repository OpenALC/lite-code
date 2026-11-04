// 大文件分级策略（ARCHITECTURE.md §5.1 表格）：输出策略对象供编辑器组件消费。
// 纯函数：输入字节大小，输出该档位下 Monaco 各能力的开/关。无框架依赖（UI.md §8）。
//
// | 文件大小  | 策略                                            |
// | < 5 MB   | 完整能力                                         |
// | 5–20 MB  | 关闭 occurrences、折叠计算、minimap，保留着色      |
// | > 20 MB  | 只读预览 + "用系统编辑器打开"                      |
//
// 边界归档：5MB 恰好落在降级档（"< 5MB" 是开区间）；20MB 恰好仍在降级档
// （"5–20 MB" 闭区间），20MB+1 字节才进入只读预览。

export const MB = 1024 * 1024

/** 完整能力档上界（开区间）：sizeBytes < FULL_LIMIT_BYTES */
export const FULL_LIMIT_BYTES = 5 * MB

/** 降级档上界（闭区间）：FULL_LIMIT_BYTES ≤ sizeBytes ≤ REDUCED_LIMIT_BYTES；超过则只读 */
export const REDUCED_LIMIT_BYTES = 20 * MB

export type LargeFileTier = 'full' | 'reduced' | 'readonly'

export interface EditorFileStrategy {
  tier: LargeFileTier
  /** 同词高亮（occurrences highlight） */
  occurrences: boolean
  /** 折叠计算 */
  folding: boolean
  /** 缩略图 minimap */
  minimap: boolean
  /** 语法着色：任何档位都不降级——表格只点名关闭 occurrences/折叠/minimap 三项 */
  syntaxColoring: boolean
  /** 只读 */
  readOnly: boolean
  /** 只读预览时提示"用系统编辑器打开" */
  suggestOpenWithSystemEditor: boolean
}

// 每档一个冻结单例：纯函数零分配，且防止消费方意外改写共享对象
const STRATEGY_FULL: EditorFileStrategy = Object.freeze({
  tier: 'full',
  occurrences: true,
  folding: true,
  minimap: true,
  syntaxColoring: true,
  readOnly: false,
  suggestOpenWithSystemEditor: false,
})

const STRATEGY_REDUCED: EditorFileStrategy = Object.freeze({
  tier: 'reduced',
  occurrences: false,
  folding: false,
  minimap: false,
  syntaxColoring: true,
  readOnly: false,
  suggestOpenWithSystemEditor: false,
})

const STRATEGY_READONLY: EditorFileStrategy = Object.freeze({
  tier: 'readonly',
  // 只读预览沿用降级档的能力裁剪：更大的文件不应反而多出能力
  occurrences: false,
  folding: false,
  minimap: false,
  syntaxColoring: true,
  readOnly: true,
  suggestOpenWithSystemEditor: true,
})

/** 按文件字节数解析编辑器降级策略。非负有限数字以外的输入抛 TypeError。 */
export function resolveEditorStrategy(sizeBytes: number): EditorFileStrategy {
  if (!Number.isFinite(sizeBytes) || sizeBytes < 0) {
    throw new TypeError(`sizeBytes 必须是非负有限数字，收到：${sizeBytes}`)
  }
  if (sizeBytes < FULL_LIMIT_BYTES) return STRATEGY_FULL
  if (sizeBytes <= REDUCED_LIMIT_BYTES) return STRATEGY_REDUCED
  return STRATEGY_READONLY
}
