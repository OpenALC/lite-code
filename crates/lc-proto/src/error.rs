//! 错误码与错误体。错误码是契约的一部分：新增为 minor，改名/删除为 major。

/// 全局错误码。跨 lane 通用，lane 私有的细粒度信息放 `ErrorBody::detail`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum ErrorCode {
    /// 未分类的内部错误
    Internal,
    /// 功能已声明但未实现（P0 骨架常用）
    NotImplemented,
    InvalidArgument,
    Timeout,
    Cancelled,
    /// 路径校验失败：目标在项目根之外（PLAN.md R3 安全边界）
    PathOutsideRoot,
    NotFound,
    Io,
    /// Node 运行时不可用（降级模式，见 docs/RUNTIME.md）
    NodeUnavailable,
    NodeBroken,
    /// 终端/命令执行依赖的 shell 不可用（docs/ARCHITECTURE.md §5.4）
    ShellUnavailable,
    ContractMismatch,
}

/// 事件流 / 命令响应统一使用的错误体。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ErrorBody {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}
