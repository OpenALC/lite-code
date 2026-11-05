//! 工具注册表骨架（docs/AGENT.md §4）。
//!
//! 工具数量上限与静态风险等级是权限闸门的输入；执行实现由后续 lane 接入。
//! 名字用 `String` 以容纳 MCP 动态工具。

use thiserror::Error;

/// 工具静态风险等级（docs/AGENT.md §5.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Risk {
    /// 只读：read_file / glob / grep / search_code
    Read,
    /// 写：write_file / edit_file / todo_write
    Write,
    /// 执行：shell / ssh_exec / task
    Execute,
    /// 网络/外部：db_query / mcp_*
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub risk: Risk,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, risk: Risk) -> Self {
        Self {
            name: name.into(),
            risk,
        }
    }
}

/// harness 的工具上限（docs/AGENT.md §4 / PLAN R6：≤12）。
pub const MAX_TOOLS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ToolRegistryError {
    #[error("工具已注册：{0}")]
    Duplicate(String),
    #[error("工具数量超过上限 {max}")]
    TooMany { max: usize },
}

#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    specs: Vec<ToolSpec>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 默认工具集，恰好 12 个；MCP 工具后续替换 `mcp_tool` 占位。
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for (name, risk) in [
            ("read_file", Risk::Read),
            ("glob", Risk::Read),
            ("grep", Risk::Read),
            ("search_code", Risk::Read),
            ("write_file", Risk::Write),
            ("edit_file", Risk::Write),
            ("todo_write", Risk::Write),
            ("shell", Risk::Execute),
            ("ssh_exec", Risk::Execute),
            ("task", Risk::Execute),
            ("db_query", Risk::Network),
            ("mcp_tool", Risk::Network),
        ] {
            registry
                .register(ToolSpec::new(name, risk))
                .expect("内置工具清单必须合法");
        }
        registry
    }

    pub fn register(&mut self, spec: ToolSpec) -> Result<(), ToolRegistryError> {
        if self.specs.iter().any(|s| s.name == spec.name) {
            return Err(ToolRegistryError::Duplicate(spec.name));
        }
        if self.specs.len() >= MAX_TOOLS {
            return Err(ToolRegistryError::TooMany { max: MAX_TOOLS });
        }
        self.specs.push(spec);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.specs.iter().find(|s| s.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolSpec> {
        self.specs.iter()
    }

    pub fn len(&self) -> usize {
        self.specs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_fill_tool_budget() {
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.len(), MAX_TOOLS);
        assert_eq!(registry.get("shell").map(|s| s.risk), Some(Risk::Execute));
        assert_eq!(
            registry.get("write_file").map(|s| s.risk),
            Some(Risk::Write)
        );
    }

    #[test]
    fn rejects_duplicate_and_overflow() {
        let mut registry = ToolRegistry::new();
        registry
            .register(ToolSpec::new("a", Risk::Read))
            .expect("first");
        assert_eq!(
            registry.register(ToolSpec::new("a", Risk::Read)),
            Err(ToolRegistryError::Duplicate("a".into()))
        );

        for i in 1..MAX_TOOLS {
            registry
                .register(ToolSpec::new(format!("t{i}"), Risk::Read))
                .expect("within limit");
        }
        assert_eq!(
            registry.register(ToolSpec::new("overflow", Risk::Read)),
            Err(ToolRegistryError::TooMany { max: MAX_TOOLS })
        );
    }
}
