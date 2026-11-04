import { describe, expect, it } from 'vitest'

import {
  FULL_LIMIT_BYTES,
  MB,
  REDUCED_LIMIT_BYTES,
  resolveEditorStrategy,
} from './largeFile'

describe('档位边界（ARCHITECTURE.md §5.1）', () => {
  it('恰好 5MB 的两侧：5MB-1 完整，5MB 整进入降级档（"< 5MB" 是开区间）', () => {
    expect(resolveEditorStrategy(FULL_LIMIT_BYTES - 1).tier).toBe('full')
    expect(resolveEditorStrategy(FULL_LIMIT_BYTES).tier).toBe('reduced')
  })

  it('恰好 20MB 的两侧：20MB 整仍在降级档（"5–20 MB" 闭区间），20MB+1 只读', () => {
    expect(resolveEditorStrategy(REDUCED_LIMIT_BYTES).tier).toBe('reduced')
    expect(resolveEditorStrategy(REDUCED_LIMIT_BYTES + 1).tier).toBe('readonly')
  })

  it('边界常量确实是 5MB 与 20MB', () => {
    expect(FULL_LIMIT_BYTES).toBe(5 * MB)
    expect(REDUCED_LIMIT_BYTES).toBe(20 * MB)
  })
})

describe('三档典型尺寸', () => {
  it('0 字节与小文件：完整能力', () => {
    expect(resolveEditorStrategy(0).tier).toBe('full')
    expect(resolveEditorStrategy(1024).tier).toBe('full')
  })

  it('10MB：降级档', () => {
    expect(resolveEditorStrategy(10 * MB).tier).toBe('reduced')
  })

  it('25MB：只读预览', () => {
    expect(resolveEditorStrategy(25 * MB).tier).toBe('readonly')
  })
})

describe('各档策略对象', () => {
  it('完整档：所有能力开启', () => {
    expect(resolveEditorStrategy(0)).toEqual({
      tier: 'full',
      occurrences: true,
      folding: true,
      minimap: true,
      syntaxColoring: true,
      readOnly: false,
      suggestOpenWithSystemEditor: false,
    })
  })

  it('降级档（5–20MB）：关闭 occurrences/折叠/minimap，保留着色，仍可编辑', () => {
    expect(resolveEditorStrategy(8 * MB)).toEqual({
      tier: 'reduced',
      occurrences: false,
      folding: false,
      minimap: false,
      syntaxColoring: true,
      readOnly: false,
      suggestOpenWithSystemEditor: false,
    })
  })

  it('只读档（>20MB）：只读 + 提示用系统编辑器打开，着色保留', () => {
    expect(resolveEditorStrategy(64 * MB)).toEqual({
      tier: 'readonly',
      occurrences: false,
      folding: false,
      minimap: false,
      syntaxColoring: true,
      readOnly: true,
      suggestOpenWithSystemEditor: true,
    })
  })

  it('能力随文件变大单调不增（readOnly/suggest 单调不减）', () => {
    const sizes = [0, 1 * MB, 8 * MB, 30 * MB]
    const strategies = sizes.map(s => resolveEditorStrategy(s))
    for (let i = 1; i < strategies.length; i++) {
      const prev = strategies[i - 1]
      const next = strategies[i]
      expect(next.occurrences ? 1 : 0).toBeLessThanOrEqual(prev.occurrences ? 1 : 0)
      expect(next.folding ? 1 : 0).toBeLessThanOrEqual(prev.folding ? 1 : 0)
      expect(next.minimap ? 1 : 0).toBeLessThanOrEqual(prev.minimap ? 1 : 0)
      expect(next.readOnly ? 1 : 0).toBeGreaterThanOrEqual(prev.readOnly ? 1 : 0)
      expect(next.suggestOpenWithSystemEditor ? 1 : 0).toBeGreaterThanOrEqual(
        prev.suggestOpenWithSystemEditor ? 1 : 0,
      )
    }
  })

  it('返回冻结单例：防消费方意外改写共享对象', () => {
    const strategy = resolveEditorStrategy(0)
    expect(() => {
      ;(strategy as { occurrences: boolean }).occurrences = false
    }).toThrow(TypeError)
    expect(resolveEditorStrategy(1024)).toBe(resolveEditorStrategy(0))
  })
})

describe('非法输入', () => {
  it('负数 / NaN / Infinity 抛 TypeError', () => {
    expect(() => resolveEditorStrategy(-1)).toThrow(TypeError)
    expect(() => resolveEditorStrategy(Number.NaN)).toThrow(TypeError)
    expect(() => resolveEditorStrategy(Number.POSITIVE_INFINITY)).toThrow(TypeError)
  })
})
