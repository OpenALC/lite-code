//! TS 绑定生成入口（bin: codegen）。
//!
//! 运行：`cargo run --locked -p lc-proto --features codegen --bin codegen`
//! 产物：`crates/lc-proto/bindings/lc-proto.d.ts`，进仓，CI contract job
//! 重新生成后执行 `git diff --exit-code` 做漂移检查。
//!
//! 设计：所有类型声明合并进单文件，避免跨文件 import 的维护成本；
//! 契约规模（v0 约 20 个类型）单文件完全可控。

use std::fs;
use std::path::PathBuf;

use ts_rs::{Config, TS};

/// 契约类型的导出清单。新增 DTO 必须同步登记，漂移检查会兜底。
macro_rules! contract_types {
    ($cfg:expr; $($t:ty),* $(,)?) => {
        vec![$((stringify!($t), <$t as TS>::decl($cfg))),*]
    };
}

fn main() {
    // u64 在 JSON 传输里是 number，不是 bigint（serde_json 序列化语义）
    let cfg = Config::default().with_large_int("number".to_owned());

    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings");
    fs::create_dir_all(&out_dir).expect("create bindings dir");

    let types: Vec<(&'static str, String)> = contract_types!(
        &cfg;
        lc_proto::ErrorCode,
        lc_proto::ErrorBody,
        lc_proto::SemVer,
        lc_proto::NodeSource,
        lc_proto::NodeHealth,
        lc_proto::NodeFailureReason,
        lc_proto::NodeProbeAttempt,
        lc_proto::NodeRuntimeInfo,
        lc_proto::NodeStatus,
        lc_proto::ShellKind,
        lc_proto::ShellInfo,
        lc_proto::EntryKind,
        lc_proto::DirEntryDto,
        lc_proto::ContractMeta,
        lc_proto::CoreCommand,
        lc_proto::CommandEnvelope,
        lc_proto::CommandResult,
        lc_proto::CoreEvent,
        lc_proto::EventEnvelope,
    );

    let mut out = String::new();
    out.push_str("// 由 lc-proto codegen 生成，请勿手改。\n");
    out.push_str("// 重新生成：cargo run --locked -p lc-proto --features codegen --bin codegen\n");
    out.push_str("// 契约版本：");
    out.push_str(lc_proto::CONTRACT_VERSION);
    out.push_str("\n\n");

    for (name, decl) in types {
        out.push_str("// ---- ");
        out.push_str(name);
        out.push_str(" ----\n");
        out.push_str(&decl);
        if !decl.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }

    let file = out_dir.join("lc-proto.d.ts");
    fs::write(&file, &out).unwrap_or_else(|e| panic!("write {}: {e}", file.display()));
    println!("written {} ({} bytes)", file.display(), out.len());
}
