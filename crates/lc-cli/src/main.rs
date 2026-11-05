//! lc-cli 入口。子命令（PLAN.md P0 起）：
//! `node detect / set-manual / portable download / info`、
//! `shell detect`、`contract info`。
//!
//! 约定：`detect` 类命令永远退出 0（只报告，不报错——未装 Node 是合法状态）；
//! `set-manual` / `portable download` / `info` 是显式动作/读取，失败退出非 0。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use lc_cli::{
    contract_meta_json, node_info_human, node_info_json, node_status_human, node_status_json,
    set_manual_report_json, shell_report_json,
};
use lc_core::config::{JsonConfigStore, NodeRuntime, load_node_runtime};
use lc_core::runtime::manual::{self, ManualError};
use lc_core::runtime::portable::{self, VersionSpec};
use lc_core::runtime::{detect_shell_real, probe_real};
use lc_proto::ContractMeta;

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
