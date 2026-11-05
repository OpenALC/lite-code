//! lc-cli 入口。子命令（PLAN.md P0 起）：
//! `node detect / set-manual / portable download / info`、
//! `shell detect`、`contract info`。
//!
//! 约定：`detect` 类命令永远退出 0（只报告，不报错——未装 Node 是合法状态）；
//! `set-manual` / `portable download` / `info` 是显式动作/读取，失败退出非 0。

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::{Parser, Subcommand};

use lc_cli::agent_run::{
    ApprovalAction, EXIT_ERROR, HumanOutput, JsonEventLines, approval_action, approval_auto_line,
    approval_details, build_session, compaction_event, error_exit_code, human_output,
    loop_error_event, mode_from_str, outcome_exit_code, resolve_instructions, resolve_workspace,
    tool_result_event,
};
use lc_cli::{
    command_result_json, contract_meta_json, node_info_human, node_info_json, node_status_human,
    node_status_json, set_manual_report_json, shell_report_json,
};
use lc_core::agent::{
    AgentLoop, CompactionObserver, DEFAULT_MAX_TURNS, LocalToolExecutor, LoopOutcome, ToolError,
    ToolOutput, ToolRegistry, ToolResultObserver, llm_event, loop_outcome,
};
use lc_core::config::{JsonConfigStore, NodeRuntime, load_node_runtime};
use lc_core::fs::{RealPathResolver, read_dir_level};
use lc_core::llm::{LlmEvent, RuntimeError, load_default, providers_path};
use lc_core::runtime::manual::{self, ManualError};
use lc_core::runtime::portable::{self, VersionSpec};
use lc_core::runtime::{detect_shell_real, probe_real};
use lc_proto::{CommandResult, ContractMeta, CoreEvent, ErrorBody, ErrorCode};

#[derive(Parser)]
#[command(
    name = "lc-cli",
    version,
    about = "Lite Code headless CLI —— lc-core 的第二个前端"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Node 运行时
    Node {
        #[command(subcommand)]
        cmd: NodeCmd,
    },
    /// Shell 探测（终端与 Agent 执行依赖）
    Shell {
        #[command(subcommand)]
        cmd: ShellCmd,
    },
    /// 契约信息
    Contract {
        #[command(subcommand)]
        cmd: ContractCmd,
    },
    /// 文件系统（headless 验证与性能基线）
    Fs {
        #[command(subcommand)]
        cmd: FsCmd,
    },
    /// Agent 会话（headless）
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
    },
}

#[derive(Subcommand)]
enum AgentCmd {
    /// 运行一次 headless Agent 会话（P3 出口命令）
    #[command(long_about = "运行一次 headless Agent 会话。\n\n\
退出码：0 = 正常结束；1 = 运行错误（含轮次上限）；2 = 权限策略拒绝或审批未通过。\n\
--mode yolo 表示完全自主；命令行调用本身即视为显式确认，无需再次确认。")]
    Run {
        /// 用户提示词
        #[arg(long)]
        prompt: String,
        /// 权限模式：plan / chat / auto / yolo（yolo 由命令行显式确认）
        #[arg(
            long,
            default_value = "auto",
            value_parser = ["plan", "chat", "auto", "yolo"]
        )]
        mode: String,
        /// 工作区根（缺省当前目录；canonicalize 后必须是已存在的目录）
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// 最大轮次（LLM 往返次数）
        #[arg(long, default_value_t = DEFAULT_MAX_TURNS)]
        max_turns: u32,
        /// 覆盖默认系统指令
        #[arg(long)]
        instructions: Option<String>,
        /// stdout 每行输出一个 EventEnvelope JSON（不混入人类文本）
        #[arg(long)]
        json: bool,
        /// 自动允许所有审批（AllowOnce），适合 CI
        #[arg(long)]
        yes: bool,
        /// 把模型的思考增量写到 stderr
        #[arg(long)]
        show_thinking: bool,
    },
}

#[derive(Subcommand)]
enum NodeCmd {
    /// 枚举候选并校验，输出 Ready / NeedManual
    Detect {
        /// 输出 JSON（契约 DTO 原样）
        #[arg(long)]
        json: bool,
    },
    /// 手动指定 node 可执行文件（docs/RUNTIME.md §5：校验通过才写 config，
    /// 永不写系统 PATH / 注册表）
    SetManual {
        /// node 可执行文件绝对路径（如 C:\Program Files\nodejs\node.exe）
        path: PathBuf,
        /// 输出 JSON 报告
        #[arg(long)]
        json: bool,
    },
    /// 便携版下载与安装
    Portable {
        #[command(subcommand)]
        cmd: PortableCmd,
    },
    /// 读取 config 中已解析的 Node 运行时（docs/RUNTIME.md §5 写入的目标）
    Info {
        /// 输出 JSON（{"node": {...} | null}）
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PortableCmd {
    /// 从官方源（或 --mirror）下载便携版 Node，SHASUMS256 校验后安装
    Download {
        /// 完整版本（22.14.0）或主版本（22 → latest-v22.x）；
        /// 缺省 22（基准版本，docs/RUNTIME.md §2/§5）
        #[arg(long)]
        version: Option<String>,
        /// 分发镜像前缀（默认 https://nodejs.org/dist）
        #[arg(long)]
        mirror: Option<String>,
    },
}

#[derive(Subcommand)]
enum ShellCmd {
    /// 探测 PowerShell 7 / bash
    Detect {
        /// 输出 JSON（{"shell": ... | null}）
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ContractCmd {
    /// 契约版本、命令、事件、错误码清单
    Info {
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FsCmd {
    /// 按层列目录（与 UI 文件树同一实现）
    ReadDir {
        /// 目录路径；缺省为 --root
        path: Option<PathBuf>,
        /// 安全边界根（缺省当前目录）
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 输出 JSON（契约 CommandResult）
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Node {
            cmd: NodeCmd::Detect { json },
        } => {
            let status = probe_real();
            if json {
                print!("{}", node_status_json(&status));
            } else {
                println!("{}", node_status_human(&status));
            }
        }
        Cmd::Node {
            cmd: NodeCmd::SetManual { path, json },
        } => {
            let res = manual::set_manual_real(&path);
            if json {
                // 失败也输出 JSON 报告（e2e 可断言 reason / detail）
                print!("{}", set_manual_report_json(&res));
            } else {
                match &res {
                    Ok(rt) => println!(
                        "node set-manual: Node.js v{} — 可以使用\n  path: {}",
                        rt.version, rt.path
                    ),
                    Err(ManualError::Invalid(reason, detail)) => {
                        eprintln!("node set-manual: 校验失败（{reason:?}）");
                        if let Some(d) = detail {
                            eprintln!("  {d}");
                        }
                    }
                    Err(ManualError::Persist(e)) => eprintln!("node set-manual: {e}"),
                }
            }
            if res.is_err() {
                std::process::exit(1);
            }
        }
        Cmd::Node {
            cmd:
                NodeCmd::Portable {
                    cmd: PortableCmd::Download { version, mirror },
                },
        } => {
            let spec = match version.as_deref() {
                Some(v) => match portable::parse_version_arg(v) {
                    Some(s) => s,
                    None => {
                        eprintln!("node portable download: 无法解析版本 {v:?}（用 22.14.0 或 22）");
                        std::process::exit(2);
                    }
                },
                // 缺省：基准 Node 22 的最新版（docs/RUNTIME.md §2/§5）
                None => VersionSpec::Latest(portable::DEFAULT_MAJOR),
            };
            match portable::install_real(&spec, mirror.as_deref()) {
                Ok(rt) => println!(
                    "node portable download: Node.js v{} — 可以使用\n  安装位置: {}\n  已写入 config",
                    rt.version, rt.path
                ),
                Err(e) => {
                    eprintln!("node portable download: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Node {
            cmd: NodeCmd::Info { json },
        } => match node_info_from_default_config() {
            Ok(rt) => {
                if json {
                    print!("{}", node_info_json(rt.as_ref()));
                } else {
                    println!("{}", node_info_human(rt.as_ref()));
                }
            }
            Err(e) => {
                eprintln!("node info: {e}");
                std::process::exit(1);
            }
        },
        Cmd::Shell {
            cmd: ShellCmd::Detect { json },
        } => {
            let shell = detect_shell_real();
            if json {
                print!("{}", shell_report_json(shell.as_ref()));
            } else {
                match shell {
                    Some(s) => println!(
                        "shell detect: {:?} {} @ {}",
                        s.kind, s.version_major, s.path
                    ),
                    None => println!(
                        "shell detect: 未找到可用 shell。Lite Code 的终端与命令执行依赖 PowerShell 7：\n  winget install --id Microsoft.PowerShell -e --source winget"
                    ),
                }
            }
        }
        Cmd::Fs {
            cmd: FsCmd::ReadDir { path, root, json },
        } => {
            let target = path.unwrap_or_else(|| root.clone());
            match read_dir_level(&root, &target, &RealPathResolver) {
                Ok(level) => {
                    if json {
                        let result = CommandResult::DirListing {
                            path: level.path,
                            entries: level.entries,
                            generation: 0,
                        };
                        print!("{}", command_result_json(&result));
                    } else {
                        println!("fs read-dir: {}", level.path);
                        for entry in level.entries {
                            let size = entry
                                .size
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "-".into());
                            println!("  {:?}\t{}\t{}", entry.kind, entry.name, size);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("fs read-dir: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Agent {
            cmd:
                AgentCmd::Run {
                    prompt,
                    mode,
                    workspace,
                    max_turns,
                    instructions,
                    json,
                    yes,
                    show_thinking,
                },
        } => {
            let code = run_agent(
                prompt,
                &mode,
                workspace,
                max_turns,
                instructions,
                json,
                yes,
                show_thinking,
            );
            if code != 0 {
                std::process::exit(code);
            }
        }
        Cmd::Contract {
            cmd: ContractCmd::Info { json },
        } => {
            let meta = ContractMeta::current();
            if json {
                print!("{}", contract_meta_json(&meta));
            } else {
                println!(
                    "lc-proto v{}：{} 命令 / {} 事件 / {} 错误码",
                    meta.contract_version,
                    meta.commands.len(),
                    meta.events.len(),
                    meta.error_codes.len()
                );
                for c in meta.commands {
                    println!("  cmd   {c}");
                }
                for e in meta.events {
                    println!("  event {e}");
                }
            }
        }
    }
}

/// `node info`：读默认路径的 config（~/.lite-code/config.json）。
fn node_info_from_default_config() -> Result<Option<NodeRuntime>, lc_core::config::ConfigError> {
    let store = JsonConfigStore::at_default_path()?;
    load_node_runtime(&store)
}

#[allow(clippy::too_many_arguments)]
fn run_agent(
    prompt: String,
    mode: &str,
    workspace: Option<PathBuf>,
    max_turns: u32,
    instructions: Option<String>,
    json: bool,
    yes: bool,
    show_thinking: bool,
) -> i32 {
    let emitter = Arc::new(Mutex::new(Emitter::new(json, show_thinking)));

    let workspace_path = workspace.unwrap_or_else(|| PathBuf::from("."));
    let workspace = match resolve_workspace(&workspace_path) {
        Ok(path) => path,
        Err(error) => {
            return fatal(
                &emitter,
                json,
                ErrorCode::InvalidArgument,
                format!("agent run: {error}"),
            );
        }
    };

    let Some(mode) = mode_from_str(mode) else {
        return fatal(
            &emitter,
            json,
            ErrorCode::InvalidArgument,
            format!("agent run: 无法解析权限模式 {mode:?}"),
        );
    };

    let runtime = match load_default() {
        Ok(runtime) => runtime,
        Err(error) => {
            return fatal(
                &emitter,
                json,
                ErrorCode::Internal,
                provider_failure_message(&error),
            );
        }
    };
    let client = match runtime.openai_client() {
        Ok(client) => client,
        Err(error) => {
            return fatal(
                &emitter,
                json,
                ErrorCode::Internal,
                provider_failure_message(&error),
            );
        }
    };

    let session_id = format!("cli-{}", std::process::id());
    let registry = ToolRegistry::with_builtins();
    let tools = registry.llm_tools();
    let session = build_session(session_id.clone(), mode, max_turns)
        .with_context(runtime.context_k, runtime.compress_at_k);
    let mut agent_loop = AgentLoop::new(
        session,
        Box::new(client),
        Box::new(LocalToolExecutor::new(workspace.clone())),
        registry,
        workspace,
        runtime.model_id,
    )
    .with_instructions(resolve_instructions(instructions.as_deref()))
    .with_tools(tools);

    let compaction_emitter = Arc::clone(&emitter);
    let compaction_session = session_id.clone();
    let compaction_observer: CompactionObserver = Box::new(move |record| {
        emit(
            &compaction_emitter,
            &compaction_event(&compaction_session, record),
        );
    });
    agent_loop = agent_loop.with_compaction_observer(compaction_observer);

    let observer_emitter = Arc::clone(&emitter);
    let observer_session = session_id.clone();
    let observer: ToolResultObserver = Box::new(
        move |call_id: &str, result: &Result<ToolOutput, ToolError>| {
            let event = tool_result_event(&observer_session, call_id, result);
            emit(&observer_emitter, &event);
        },
    );
    agent_loop = agent_loop.with_tool_observer(observer);

    let sink_emitter = Arc::clone(&emitter);
    let sink_session = session_id.clone();
    let mut sink = move |event: LlmEvent| {
        if let Some(event) = llm_event(&sink_session, &event) {
            emit(&sink_emitter, &event);
        }
    };

    let mut outcome = Some(agent_loop.run(&prompt, &mut sink));
    let mut denied_by_cli = false;
    let exit_code = loop {
        let current = match outcome.take() {
            Some(Ok(current)) => current,
            Some(Err(error)) => {
                emit(&emitter, &loop_error_event(&error));
                break error_exit_code();
            }
            None => unreachable!("每轮都会重新赋值 outcome"),
        };
        let events = loop_outcome(&session_id, &current);
        let code = outcome_exit_code(&current);
        match current {
            LoopOutcome::Final { .. } => {
                for event in &events {
                    emit(&emitter, event);
                }
                break code;
            }
            LoopOutcome::Denied { .. } => {
                // 审批被 CLI 拒绝时，完整字段已写 stderr；JSON 仍要 error 事件。
                if !denied_by_cli || json {
                    for event in &events {
                        emit(&emitter, event);
                    }
                }
                break code;
            }
            LoopOutcome::NeedsApproval { approval } => {
                for event in &events {
                    emit(&emitter, event);
                }
                let action = approval_action(yes);
                if action == ApprovalAction::Deny {
                    denied_by_cli = true;
                }
                if !json {
                    match action {
                        ApprovalAction::AllowOnce => {
                            eprint!("{}", approval_auto_line(&approval));
                        }
                        ApprovalAction::Deny => {
                            eprint!("{}", approval_details(&approval));
                        }
                    }
                }
                outcome =
                    Some(agent_loop.resolve_approval(approval.id, action.decision(), &mut sink));
            }
        }
    };

    emitter.lock().expect("emitter mutex").finish();
    exit_code
}

fn emit(emitter: &Arc<Mutex<Emitter>>, event: &CoreEvent) {
    emitter.lock().expect("emitter mutex").emit(event);
}

fn fatal(emitter: &Arc<Mutex<Emitter>>, json: bool, code: ErrorCode, message: String) -> i32 {
    emit(
        emitter,
        &CoreEvent::Error {
            error: ErrorBody::new(code, message.clone()),
        },
    );
    if json {
        // JSON 模式 stdout 只有事件；可操作提示照常写 stderr。
        eprintln!("{message}");
    }
    EXIT_ERROR
}

fn provider_failure_message(error: &RuntimeError) -> String {
    let path = providers_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "~/.lite-code/providers.json".into());
    format!(
        "agent run: provider 不可用：{error}\n\
         \n\
         providers 配置文件：{path}\n\
         可用环境变量 LC_PROVIDERS_FILE 指向其他文件。\n\
         密钥优先读取 LC_API_KEY；providers.json 中也可用 \"apiKeyRef\": \"env:NAME\" 引用环境变量。\n\
         没有配置文件时可只设环境变量：LC_API_KEY（必填）、LC_BASE_URL、LC_MODEL。"
    )
}

/// 事件出口：JSON 模式一行一个 EventEnvelope；人类模式按流写 stdout/stderr。
struct Emitter {
    json: bool,
    show_thinking: bool,
    lines: JsonEventLines,
    wrote_stdout: bool,
}

impl Emitter {
    fn new(json: bool, show_thinking: bool) -> Self {
        Self {
            json,
            show_thinking,
            lines: JsonEventLines::new(),
            wrote_stdout: false,
        }
    }

    fn emit(&mut self, event: &CoreEvent) {
        if self.json {
            let line = self.lines.next_line(event);
            let mut stdout = io::stdout().lock();
            let _ = writeln!(stdout, "{line}");
            let _ = stdout.flush();
            return;
        }

        match human_output(event, self.show_thinking) {
            HumanOutput::Nothing => {}
            HumanOutput::Stdout(text) => {
                self.wrote_stdout |= !text.is_empty();
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            HumanOutput::Stderr(text) => {
                let mut stderr = io::stderr().lock();
                let _ = stderr.write_all(text.as_bytes());
                let _ = stderr.flush();
            }
        }
    }

    fn finish(&mut self) {
        if !self.json && self.wrote_stdout {
            let mut stdout = io::stdout().lock();
            let _ = writeln!(stdout);
            let _ = stdout.flush();
        }
    }
}
