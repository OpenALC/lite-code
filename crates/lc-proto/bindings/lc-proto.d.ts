// 由 lc-proto codegen 生成，请勿手改。
// 重新生成：cargo run --locked -p lc-proto --features codegen --bin codegen
// 契约版本：0.8.0

// ---- lc_proto::ErrorCode ----
export type ErrorCode = "internal" | "not_implemented" | "invalid_argument" | "timeout" | "cancelled" | "path_outside_root" | "not_found" | "io" | "node_unavailable" | "node_broken" | "shell_unavailable" | "contract_mismatch" | "permission_denied" | "db_unavailable" | "db_rejected" | "db_read_only" | "git_unavailable" | "git_rejected";

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

// ---- lc_proto::DbKind ----
export type DbKind = "postgres" | "mysql" | "mssql" | "sqlite";

// ---- lc_proto::DbConnectionState ----
export type DbConnectionState = "disconnected" | "connecting" | "connected" | "error";

// ---- lc_proto::DbConnectionInfo ----
export type DbConnectionInfo = { connection_id: string, name: string, kind: DbKind, read_only: boolean, state: DbConnectionState, host?: string | null, database?: string | null, error?: string | null, };

// ---- lc_proto::DbSchemaNodeKind ----
export type DbSchemaNodeKind = "database" | "schema" | "table" | "view" | "column";

// ---- lc_proto::DbSchemaNode ----
export type DbSchemaNode = { name: string, kind: DbSchemaNodeKind, detail?: string | null, children: Array<DbSchemaNode>, };

// ---- lc_proto::DbColumn ----
export type DbColumn = { name: string, type_name: string, nullable: boolean, };

// ---- lc_proto::DbQueryResult ----
export type DbQueryResult = { columns: Array<DbColumn>, rows: Array<Array<string | null>>, row_count: number, truncated: boolean, handle?: string | null, elapsed_ms: number, 
/**
 * 数据库内容不可信，前端不得当指令执行（P5 出口 #4）。
 */
untrusted: boolean, };

// ---- lc_proto::DbExecResult ----
export type DbExecResult = { affected_rows: number, elapsed_ms: number, };

// ---- lc_proto::DbExplainResult ----
export type DbExplainResult = { plan: string, elapsed_ms: number, };

// ---- lc_proto::DbConnectionConfig ----
export type DbConnectionConfig = { connection_id: string, name: string, kind: DbKind, host: string, port: number, database: string, user: string, 
/**
 * `keyring://...` / `env:NAME`；空串表示 SQLite 或无认证。
 */
secret_ref: string, read_only: boolean, extra?: string | null, };

// ---- lc_proto::GitFileStatus ----
export type GitFileStatus = { path: string, status: ChangeStatus, staged: boolean, added: number, removed: number, };

// ---- lc_proto::GitStatus ----
export type GitStatus = { branch?: string | null, upstream?: string | null, ahead: number, behind: number, clean: boolean, merge_in_progress: boolean, rebase_in_progress: boolean, files: Array<GitFileStatus>, };

// ---- lc_proto::GitCommit ----
export type GitCommit = { sha: string, short_sha: string, author: string, 
/**
 * 第一行摘要（完整 message 不进列表，详情按需读取）。
 */
message: string, 
/**
 * Unix 秒。
 */
timestamp: number, };

// ---- lc_proto::LspCompletionItem ----
export type LspCompletionItem = { label: string, 
/**
 * LSP CompletionItemKind（1–25）；缺省或越界时前端按 Text 处理。
 */
kind?: number | null, detail?: string | null, 
/**
 * 缺省时前端用 label 作为插入文本。
 */
insertText?: string | null, 
/**
 * plain insert 文本编辑。v0 只取 `newText`；编辑范围由编辑器按当前词
 * 计算（textEdit 存在时优先于 insertText）。
 */
textEdit?: LspTextEdit | null, };

// ---- lc_proto::LspTextEdit ----
export type LspTextEdit = { newText: string, };

// ---- lc_proto::LspHover ----
export type LspHover = { contents: Array<LspHoverContent>, };

// ---- lc_proto::LspHoverContent ----
export type LspHoverContent = { value: string, isMarkdown: boolean, };

// ---- lc_proto::LspLocation ----
export type LspLocation = { path: string, line: number, column: number, };

// ---- lc_proto::CoreCommand ----
export type CoreCommand = { "method": "node_detect" } | { "method": "node_set_manual", path: string, } | { "method": "node_download_portable", version: string | null, } | { "method": "shell_detect" } | { "method": "workspace_root" } | { "method": "workspace_open", path: string, } | { "method": "fs_read_dir", path: string, } | { "method": "fs_read_file", path: string, } | { "method": "fs_write_file", path: string, content: string, } | { "method": "session_create", mode: SessionMode, task_id?: string | null, } | { "method": "agent_send", session_id: string, text: string, } | { "method": "agent_cancel", session_id: string, } | { "method": "approval_resolve", session_id: string, approval_id: number, decision: ApprovalDecision, } | { "method": "task_list" } | { "method": "task_create", title: string, session_id: string | null, } | { "method": "task_start", task_id: string, } | { "method": "task_cancel", task_id: string, } | { "method": "task_apply", task_id: string, } | { "method": "task_discard", task_id: string, } | { "method": "task_diff", task_id: string, } | { "method": "task_bind_session", task_id: string, session_id: string | null, } | { "method": "db_list" } | { "method": "db_connect", connection_id: string, } | { "method": "db_disconnect", connection_id: string, } | { "method": "db_schema", connection_id: string, } | { "method": "db_query", connection_id: string, sql: string, limit: number | null, } | { "method": "db_execute", connection_id: string, sql: string, } | { "method": "db_explain", connection_id: string, sql: string, } | { "method": "db_save_connection", connection: DbConnectionConfig, } | { "method": "db_delete_connection", connection_id: string, } | { "method": "git_status" } | { "method": "git_diff", path: string | null, staged: boolean, } | { "method": "git_stage", paths: Array<string>, } | { "method": "git_unstage", paths: Array<string>, } | { "method": "git_commit", message: string, } | { "method": "git_log", limit: number | null, } | { "method": "contract_info" };

// ---- lc_proto::CommandEnvelope ----
export type CommandEnvelope = { id: number, command: CoreCommand, };

// ---- lc_proto::CommandResult ----
export type CommandResult = { "kind": "node_status", status: NodeStatus, } | { "kind": "shell", shell: ShellInfo | null, } | { "kind": "session", session: SessionInfo, } | { "kind": "tasks", tasks: Array<TaskInfo>, } | { "kind": "task", task: TaskInfo, } | { "kind": "task_diff", task_id: string, patch: string, files: Array<FileChangeDto>, } | { "kind": "connections", connections: Array<DbConnectionInfo>, } | { "kind": "connection", connection: DbConnectionInfo, } | { "kind": "db_schema", connection_id: string, nodes: Array<DbSchemaNode>, } | { "kind": "db_query", result: DbQueryResult, } | { "kind": "db_exec", result: DbExecResult, } | { "kind": "db_explain", result: DbExplainResult, } | { "kind": "git_status", status: GitStatus, } | { "kind": "git_diff", patch: string, files: Array<GitFileStatus>, } | { "kind": "git_log", commits: Array<GitCommit>, } | { "kind": "workspace_root", root: string | null, } | { "kind": "dir_listing", path: string, entries: Array<DirEntryDto>, generation: number, } | { "kind": "file_content", path: string, text: string, } | { "kind": "contract", meta: ContractMeta, } | { "kind": "ack" };

// ---- lc_proto::CoreEvent ----
export type CoreEvent = { "type": "node:status", status: NodeStatus, } | { "type": "shell:status", shell: ShellInfo | null, } | { "type": "fs:tree", generation: number, } | { "type": "fs:entry", path: string, } | { "type": "task:created", task_id: string, title: string, } | { "type": "task:status", task_id: string, status: string, } | { "type": "task:updated", task: TaskInfo, } | { "type": "task:step", task_id: string, index: number, title: string, state: TaskStepState, } | { "type": "task:change", task_id: string, change: FileChangeDto, } | { "type": "task:check", task_id: string, check: TaskCheck, } | { "type": "task:discarded", task_id: string, } | { "type": "db:status", connection: DbConnectionInfo, } | { "type": "task:needs_input", task_id: string, kind: string, } | { "type": "task:merged", task_id: string, commit_sha: string, } | { "type": "agent:text_delta", session_id: string, index: number, text: string, } | { "type": "agent:thinking_delta", session_id: string, text: string, } | { "type": "agent:tool_start", session_id: string, call_id: string, name: string, } | { "type": "agent:tool_result", session_id: string, call_id: string, ok: boolean, text: string, } | { "type": "agent:approval_required", session_id: string, approval: ApprovalPrompt, } | { "type": "agent:session_state", session: SessionInfo, } | { "type": "agent:usage", session_id: string, input: number, output: number, cached: number, } | { "type": "agent:compacted", session_id: string, record_id: number, before_tokens: number, after_tokens: number, removed_messages: number, summary: string, } | { "type": "agent:done", session_id: string, stop_reason: string, } | { "type": "error", error: ErrorBody, };

// ---- lc_proto::EventEnvelope ----
export type EventEnvelope = { seq: number, event: CoreEvent, };

