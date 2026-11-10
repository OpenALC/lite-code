//! LSP 编辑器特性请求 DTO（docs/EXTENSIONS.md §3）。
//!
//! 补全 / 悬停 / 跳转的请求-响应命令：`lsp_completion` / `lsp_hover` /
//! `lsp_definition`。参数统一为 `path` + 1-based `line` / `column`（与 Monaco
//! 及诊断 DTO 的坐标约定一致），lc-core 负责换算成 LSP 的 0-based UTF-16
//! position；服务器未启动 / 文件未 didOpen / 超时 / server 返回 error 一律
//! 走统一的 `ErrorBody` 结构错误。

use serde::{Deserialize, Serialize};

/// LSP CompletionItem 的 v0 子集。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct LspCompletionItem {
    pub label: String,
    /// LSP CompletionItemKind（1–25）；缺省或越界时前端按 Text 处理。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 缺省时前端用 label 作为插入文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<String>,
    /// plain insert 文本编辑。v0 只取 `newText`；编辑范围由编辑器按当前词
    /// 计算（textEdit 存在时优先于 insertText）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_edit: Option<LspTextEdit>,
}

/// plain insert 文本编辑（LSP TextEdit / InsertReplaceEdit 的 v0 子集）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct LspTextEdit {
    pub new_text: String,
}

/// 悬停结果；`contents` 为空表示该位置无悬停。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct LspHover {
    pub contents: Vec<LspHoverContent>,
}

/// 一段悬停内容。`value` 已拍平为单段文本：LSP MarkupContent 原样保留，
/// MarkedString 的字符串形式视为 markdown，`{language, value}` 形式由
/// lc-core 包成围栏代码块。`is_markdown = false` 表示纯文本段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct LspHoverContent {
    pub value: String,
    pub is_markdown: bool,
}

/// 跳转目标位置；行列 1-based（与 Monaco 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct LspLocation {
    pub path: String,
    pub line: u32,
    pub column: u32,
}
