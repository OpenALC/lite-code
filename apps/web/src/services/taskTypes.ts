// Task/工作台 DTO（docs/TASKS.md §2）。
//
// 类型单一来源：@lc-proto（crates/lc-proto codegen 产物）。本文件只保留
// 运行时形状校验与任务状态枚举数组，字段名/枚举值与 Rust serde 严格一致。

import type {
  ChangeStatus,
  FileChangeDto,
  TaskCheck,
  TaskInfo,
  TaskStatus,
  TaskStep,
  TaskStepState,
} from '@lc-proto'

export type {
  ChangeStatus,
  FileChangeDto,
  TaskCheck,
  TaskInfo,
  TaskStatus,
  TaskStep,
  TaskStepState,
}

export const TASK_STATUSES = [
  'planning',
  'running',
  'blocked',
  'review',
  'done',
  'failed',
  'queued',
] as const

export const CHANGE_STATUSES = ['modified', 'added', 'deleted', 'renamed'] as const

export const TASK_STEP_STATES = ['pending', 'running', 'done', 'failed'] as const

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isString(value: unknown): value is string {
  return typeof value === 'string'
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

/** Rust Option<String> 序列化为 null；兼容省略字段的旧载荷。 */
function isOptionalString(value: unknown): value is string | null | undefined {
  return value === null || value === undefined || typeof value === 'string'
}

function isOptionalPort(value: unknown): value is number | null | undefined {
  return (
    value === null ||
    value === undefined ||
    (isFiniteNumber(value) && Number.isInteger(value) && value >= 0 && value <= 65535)
  )
}

function isNonNegative(value: unknown): value is number {
  return isFiniteNumber(value) && value >= 0
}

export function isTaskStatus(value: unknown): value is TaskStatus {
  return typeof value === 'string' && (TASK_STATUSES as readonly string[]).includes(value)
}

export function isChangeStatus(value: unknown): value is ChangeStatus {
  return typeof value === 'string' && (CHANGE_STATUSES as readonly string[]).includes(value)
}

export function isTaskStepState(value: unknown): value is TaskStepState {
  return typeof value === 'string' && (TASK_STEP_STATES as readonly string[]).includes(value)
}

export function isTaskStep(value: unknown): value is TaskStep {
  if (!isRecord(value)) return false
  return (
    isNonNegative(value.index) &&
    Number.isInteger(value.index) &&
    isString(value.title) &&
    isTaskStepState(value.state)
  )
}

export function isFileChangeDto(value: unknown): value is FileChangeDto {
  if (!isRecord(value)) return false
  return (
    isString(value.path) &&
    isChangeStatus(value.status) &&
    typeof value.applied === 'boolean' &&
    isNonNegative(value.added) &&
    isNonNegative(value.removed)
  )
}

export function isTaskCheck(value: unknown): value is TaskCheck {
  if (!isRecord(value)) return false
  return (
    isString(value.name) &&
    typeof value.ok === 'boolean' &&
    isString(value.output_tail) &&
    isNonNegative(value.duration_ms)
  )
}

/**
 * TaskInfo 全量形状校验。IPC 对端不可信：任何缺字段/错枚举的载荷都
 * 从事件流或命令结果里丢弃，避免 reducer 在渲染时炸掉。
 */
export function isTaskInfo(value: unknown): value is TaskInfo {
  if (!isRecord(value)) return false
  return (
    isString(value.task_id) &&
    isString(value.title) &&
    isOptionalString(value.session_id) &&
    isTaskStatus(value.status) &&
    isOptionalString(value.worktree_path) &&
    isOptionalString(value.branch) &&
    isOptionalPort(value.port) &&
    isOptionalString(value.base_sha) &&
    Array.isArray(value.steps) &&
    value.steps.every(isTaskStep) &&
    Array.isArray(value.changes) &&
    value.changes.every(isFileChangeDto) &&
    Array.isArray(value.checks) &&
    value.checks.every(isTaskCheck) &&
    isFiniteNumber(value.created_at)
  )
}
