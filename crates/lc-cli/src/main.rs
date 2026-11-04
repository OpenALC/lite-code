//! lc-cli 入口。三个示例命令（PLAN.md P0）：
//! `node detect` / `shell detect` / `contract info`。
//!
//! 约定：探测类命令永远退出 0（只报告，不报错——未装 Node 是合法状态）。

use clap::{Parser, Subcommand};

use lc_cli::{contract_meta_json, node_status_human, node_status_json, shell_report_json};
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
    /// Node 运行时探测
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
