// 由 lc-proto codegen 生成，请勿手改。
// 重新生成：cargo run --locked -p lc-proto --features codegen --bin codegen
// 契约版本：0.5.0

// ---- lc_proto::ErrorCode ----
export type ErrorCode = "internal" | "not_implemented" | "invalid_argument" | "timeout" | "cancelled" | "path_outside_root" | "not_found" | "io" | "node_unavailable" | "node_broken" | "shell_unavailable" | "contract_mismatch" | "permission_denied";

// ---- lc_proto::ErrorBody ----
export type ErrorBody = { code: ErrorCode, message: string, detail?: string | null, };

// ---- lc_proto::SemVer ----
export type SemVer = { major: number, minor: number, patch: number, };

// ---- lc_proto::NodeSource ----
export type NodeSource = "official_installer" | "version_manager" | "package_manager" | "registry_path" | "manual" | "portable";

// ---- lc_proto::NodeHealth ----
export type NodeHealth = "supported" | "unverified" | "legacy";

// ---- lc_proto::NodeFailureReason ----
export type NodeFailureReason = "missing" | "not_a_file" | "store_alias" | "spawn_failed" | "unparsable" | "ghost_install" | "broken";

// ---- lc_proto::NodeProbeAttempt ----
export type NodeProbeAttempt = { path: string, source: NodeSource, reason: NodeFailureReason, detail?: string | null, };

// ---- lc_proto::NodeRuntimeInfo ----
export type NodeRuntimeInfo = { path: string, version: SemVer, source: NodeSource, health: NodeHealth, };

// ---- lc_proto::NodeStatus ----
export type NodeStatus = { "kind": "ready", runtime: NodeRuntimeInfo, } | { "kind": "need_manual", attempts: Array<NodeProbeAttempt>, };

// ---- lc_proto::ShellKind ----
export type ShellKind = "pwsh" | "bash";

// ---- lc_proto::ShellInfo ----
export type ShellInfo = { path: string, kind: ShellKind, 
/**
 * pwsh 要求 major ≥ 7
 */
version_major: number, };

// ---- lc_proto::EntryKind ----
export type EntryKind = "file" | "dir" | "symlink";

// ---- lc_proto::DirEntryDto ----
export type DirEntryDto = { name: string, path: string, kind: EntryKind, size?: number | null, };

// ---- lc_proto::ContractMeta ----
export type ContractMeta = { name: string, contract_version: string, commands: Array<string>, events: Array<string>, error_codes: Array<string>, };

// ---- lc_proto::SessionMode ----
export type SessionMode = "plan" | "chat" | "auto" | "yolo";

// ---- lc_proto::ApprovalDecision ----
export type ApprovalDecision = "allow_once" | "allow_session" | "deny";

// ---- lc_proto::ApprovalSource ----
export type ApprovalSource = "mode" | "static_blacklist" | "user_blacklist" | "model_risk" | "outside_workspace" | "network_tool" | "execute_review";

// ---- lc_proto::SessionInfo ----
export type SessionInfo = { session_id: string, mode: SessionMode, turn: number, max_turns: number, 
/**
 * 模型上下文窗口（k token），UI 用量条分母。
 */
context_k: number, 
/**
 * 压缩阈值（k token），UI 刻度线。
 */
compress_at_k: number, cancelled: boolean, };

// ---- lc_proto::ApprovalPrompt ----
export type ApprovalPrompt = { approval_id: number, tool: string, command?: string | null, source: ApprovalSource, reason: string, };

// ---- lc_proto::TaskStatus ----
export type TaskStatus = "planning" | "running" | "blocked" | "review" | "done" | "failed" | "queued";

// ---- lc_proto::ChangeStatus ----
export type ChangeStatus = "modified" | "added" | "deleted" | "renamed";

// ---- lc_proto::TaskStepState ----
export type TaskStepState = "pending" | "running" | "done" | "failed";

// ---- lc_proto::TaskStep ----
export type TaskStep = { index: number, title: string, state: TaskStepState, };

// ---- lc_proto::FileChangeDto ----
export type FileChangeDto = { path: string, status: ChangeStatus, applied: boolean, added: number, removed: number, };

// ---- lc_proto::TaskCheck ----
export type TaskCheck = { name: string, ok: boolean, output_tail: string, duration_ms: number, };

// ---- lc_proto::TaskInfo ----
export type TaskInfo = { task_id: string, title: string, session_id: string | null, status: TaskStatus, worktree_path: string | null, branch: string | null, port: number | null, base_sha: string | null, steps: Array<TaskStep>, changes: Array<FileChangeDto>, checks: Array<TaskCheck>, created_at: number, };

// ---- lc_proto::CoreCommand ----
export type CoreCommand = { "method": "node_detect" } | { "method": "node_set_manual", path: string, } | { "method": "node_download_portable", version: string | null, } | { "method": "shell_detect" } | { "method": "workspace_root" } | { "method": "workspace_open", path: string, } | { "method": "fs_read_dir", path: string, } | { "method": "fs_read_file", path: string, } | { "method": "fs_write_file", path: string, content: string, } | { "method": "session_create", mode: SessionMode, task_id?: string | null, } | { "method": "agent_send", session_id: string, text: string, } | { "method": "agent_cancel", session_id: string, } | { "method": "approval_resolve", session_id: string, approval_id: number, decision: ApprovalDecision, } | { "method": "task_list" } | { "method": "task_create", title: string, session_id: string | null, } | { "method": "task_start", task_id: string, } | { "method": "task_cancel", task_id: string, } | { "method": "task_apply", task_id: string, } | { "method": "task_discard", task_id: string, } | { "method": "task_diff", task_id: string, } | { "method": "task_bind_session", task_id: string, session_id: string | null, } | { "method": "contract_info" };

// ---- lc_proto::CommandEnvelope ----
export type CommandEnvelope = { id: number, command: CoreCommand, };

// ---- lc_proto::CommandResult ----
export type CommandResult = { "kind": "node_status", status: NodeStatus, } | { "kind": "shell", shell: ShellInfo | null, } | { "kind": "session", session: SessionInfo, } | { "kind": "tasks", tasks: Array<TaskInfo>, } | { "kind": "task", task: TaskInfo, } | { "kind": "task_diff", task_id: string, patch: string, files: Array<FileChangeDto>, } | { "kind": "workspace_root", root: string | null, } | { "kind": "dir_listing", path: string, entries: Array<DirEntryDto>, generation: number, } | { "kind": "file_content", path: string, text: string, } | { "kind": "contract", meta: ContractMeta, } | { "kind": "ack" };

// ---- lc_proto::CoreEvent ----
export type CoreEvent = { "type": "node:status", status: NodeStatus, } | { "type": "shell:status", shell: ShellInfo | null, } | { "type": "fs:tree", generation: number, } | { "type": "fs:entry", path: string, } | { "type": "task:created", task_id: string, title: string, } | { "type": "task:status", task_id: string, status: string, } | { "type": "task:updated", task: TaskInfo, } | { "type": "task:step", task_id: string, index: number, title: string, state: TaskStepState, } | { "type": "task:change", task_id: string, change: FileChangeDto, } | { "type": "task:check", task_id: string, check: TaskCheck, } | { "type": "task:discarded", task_id: string, } | { "type": "task:needs_input", task_id: string, kind: string, } | { "type": "task:merged", task_id: string, commit_sha: string, } | { "type": "agent:text_delta", session_id: string, index: number, text: string, } | { "type": "agent:thinking_delta", session_id: string, text: string, } | { "type": "agent:tool_start", session_id: string, call_id: string, name: string, } | { "type": "agent:tool_result", session_id: string, call_id: string, ok: boolean, text: string, } | { "type": "agent:approval_required", session_id: string, approval: ApprovalPrompt, } | { "type": "agent:session_state", session: SessionInfo, } | { "type": "agent:usage", session_id: string, input: number, output: number, cached: number, } | { "type": "agent:compacted", session_id: string, record_id: number, before_tokens: number, after_tokens: number, removed_messages: number, summary: string, } | { "type": "agent:done", session_id: string, stop_reason: string, } | { "type": "error", error: ErrorBody, };

// ---- lc_proto::EventEnvelope ----
export type EventEnvelope = { seq: number, event: CoreEvent, };

