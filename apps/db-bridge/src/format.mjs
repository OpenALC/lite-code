// 结果单元格与行数上限的归一化。数据库返回的任意类型都必须能安全地变成
// JSON 字符串；Buffer/Date/BigInt 等不能依赖默认 String()。

import { invalidArgument } from './errors.mjs';

export const DEFAULT_QUERY_LIMIT = 100;
export const MAX_QUERY_LIMIT = 1000;

export function normalizeLimit(value) {
  if (value === undefined || value === null) return DEFAULT_QUERY_LIMIT;
  if (!Number.isInteger(value) || value < 0) {
    throw invalidArgument('limit must be a non-negative integer');
  }
  return Math.min(value, MAX_QUERY_LIMIT);
}

export function cell(value) {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'boolean' || typeof value === 'number') return String(value);
  if (typeof value === 'bigint') return value.toString();
  if (value instanceof Date) {
    return Number.isNaN(value.getTime()) ? null : value.toISOString();
  }
  if (Buffer.isBuffer(value) || value instanceof Uint8Array) {
    return `0x${Buffer.from(value).toString('hex')}`;
  }
  if (typeof value === 'object') {
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }
  return String(value);
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
