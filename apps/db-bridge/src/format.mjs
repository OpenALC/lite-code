// 结果单元格与行数/字节上限的归一化。数据库返回的任意类型都必须能安全地变成
// JSON 字符串；Buffer/Date/BigInt 等不能依赖默认 String()。

import { invalidArgument } from './errors.mjs';

export const DEFAULT_QUERY_LIMIT = 100;
export const MAX_QUERY_LIMIT = 1000;

// 单元格字节上限：超限截断并加省略后缀，避免单行大 TEXT 撑爆 IPC/内存。
export const MAX_CELL_BYTES = 4096;
// 结果集累计字节预算（对齐 lc-core MAX_STREAM_BYTES 的防御思路）：超限即
// 停止取行并标记 truncated。
export const MAX_RESULT_BYTES = 128 * 1024;
export const TRUNCATED_SUFFIX = '…[truncated]';

const utf8Bytes = (text) => Buffer.byteLength(text, 'utf8');

// 按 UTF-8 字节截断并追加省略后缀，保持在字符边界上（不把多字节字符切成
// 半个，避免产出非法序列）。
export function truncateText(text, maxBytes = MAX_CELL_BYTES, suffix = TRUNCATED_SUFFIX) {
  if (utf8Bytes(text) <= maxBytes) return text;
  const headBytes = Math.max(0, maxBytes - utf8Bytes(suffix));
  const buffer = Buffer.from(text, 'utf8').subarray(0, headBytes);
  let end = buffer.length;
  while (end > 0 && (buffer[end - 1] & 0xc0) === 0x80) end -= 1;
  // 末尾若残留半个多字节字符的首字节，也一并去掉。
  if (end > 0 && buffer[end - 1] >= 0xc0) end -= 1;
  return `${buffer.subarray(0, end).toString('utf8')}${suffix}`;
}

export function normalizeLimit(value) {
  if (value === undefined || value === null) return DEFAULT_QUERY_LIMIT;
  if (!Number.isInteger(value) || value < 0) {
    throw invalidArgument('limit must be a non-negative integer');
  }
  return Math.min(value, MAX_QUERY_LIMIT);
}

export function cell(value) {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string') return truncateText(value);
  if (typeof value === 'boolean' || typeof value === 'number') return String(value);
  if (typeof value === 'bigint') return value.toString();
  if (value instanceof Date) {
    if (Number.isNaN(value.getTime())) return null;
    return truncateText(value.toISOString());
  }
  if (Buffer.isBuffer(value) || value instanceof Uint8Array) {
    return truncateText(`0x${Buffer.from(value).toString('hex')}`);
  }
  if (typeof value === 'object') {
    try {
      return truncateText(JSON.stringify(value));
    } catch {
      return truncateText(String(value));
    }
  }
  return truncateText(String(value));
}

// 结果集字节预算：累计每个非 null 单元格的 UTF-8 字节数（截断后），超限即
// 停止取行并标记 truncated。首行永远放行，避免单行宽表把结果打成空集。
export function createResultBudget(maxBytes = MAX_RESULT_BYTES) {
  let used = 0;
  return {
    tryAdd(cells) {
      let bytes = 0;
      for (const value of cells) {
        if (value !== null && value !== undefined) bytes += utf8Bytes(value);
      }
      if (used > 0 && used + bytes > maxBytes) return false;
      used += bytes;
      return true;
    },
    usedBytes() {
      return used;
    },
  };
}

export function normalizeColumns(columns, width) {
  const normalized = (columns ?? []).map((column) => ({
    name: String(column?.name ?? ''),
    type_name: String(column?.type_name ?? ''),
    nullable: column?.nullable !== false,
  }));
  while (normalized.length < width) {
    normalized.push({
      name: `column_${normalized.length + 1}`,
      type_name: '',
      nullable: true,
    });
  }
  return normalized;
}

export function elapsed(started) {
  return Math.max(0, Math.round(performance.now() - started));
}
