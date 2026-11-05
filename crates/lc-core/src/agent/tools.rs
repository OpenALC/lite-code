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

    /// Responses API 的工具定义（本地执行器 + [`crate::db::DbExecutor`]
    /// 支持的 11 个静态工具）。CLI 与 Tauri 共用，避免两处手写 schema。
    pub fn llm_tools(&self) -> Vec<serde_json::Value> {
        let supported = [
            "read_file",
            "write_file",
            "edit_file",
            "glob",
            "grep",
            "shell",
            "db_list",
            "db_schema",
            "db_query",
            "db_explain",
            "db_execute",
        ];
        builtin_tool_definitions()
            .into_iter()
            .filter(|tool| {
                tool.get("name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|name| supported.contains(&name) && self.get(name).is_some())
            })
            .collect()
    }

    /// 默认工具集，恰好 12 个；`mcp_tool` 是动态 MCP 工具的占位，
    /// 由 [`ToolRegistry::with_mcp_tools`] 替换。
    ///
    /// P5 的 12 工具预算优先给数据库：`search_code` / `todo_write` /
    /// `ssh_exec` / `task` 占位被移出（它们的执行器尚未接入），换成
    /// 5 个 `db_*` 工具。所有数据库工具都是 `Network`，由策略闸门要求
    /// 审批；`db_execute` 另有 SQL 闸门与只读连接兜底。
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for (name, risk) in [
            ("read_file", Risk::Read),
            ("write_file", Risk::Write),
            ("edit_file", Risk::Write),
            ("glob", Risk::Read),
            ("grep", Risk::Read),
            ("shell", Risk::Execute),
            ("db_list", Risk::Network),
            ("db_schema", Risk::Network),
            ("db_query", Risk::Network),
            ("db_explain", Risk::Network),
            ("db_execute", Risk::Network),
            ("mcp_tool", Risk::Network),
        ] {
            registry
                .register(ToolSpec::new(name, risk))
                .expect("内置工具清单必须合法");
        }
        registry
    }

    /// 移除占位 `mcp_tool` 后注册动态 MCP 工具；只注册到上限，多余的静默
    /// 截断（保持 harness 的 12 工具预算）。内置的 5 个 `db_*` 工具保留。
    pub fn with_mcp_tools(mut self, specs: impl IntoIterator<Item = ToolSpec>) -> Self {
        self.specs.retain(|spec| spec.name != "mcp_tool");
        for spec in specs {
            match self.register(spec) {
                Ok(()) => {}
                Err(ToolRegistryError::Duplicate(_)) => continue,
                Err(ToolRegistryError::TooMany { .. }) => break,
            }
        }
        self
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

/// Responses API 的 11 个静态内置工具定义（6 个本地文件/shell + 5 个数据库）。
/// 名称/参数必须与 `LocalToolExecutor` / [`crate::db::DbExecutor`] 的分支
/// 保持一致；新增执行器时同步这里。`mcp_tool` 由动态 MCP 定义单独提供。
pub fn builtin_tool_definitions() -> Vec<serde_json::Value> {
    use serde_json::json;

    let mut definitions = vec![
        json!({
            "type": "function",
            "name": "read_file",
            "description": "Read a UTF-8 text file inside the workspace. Paths are relative to the workspace root.",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string", "description": "File path inside the workspace"}},
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "write_file",
            "description": "Write a UTF-8 text file inside the workspace, creating parent directories as needed.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path inside the workspace"},
                    "content": {"type": "string", "description": "Full file content"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "edit_file",
            "description": "Replace an exact string in a workspace file. By default the old string must be unique; set replace_all to replace every occurrence.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence instead of requiring a unique match"}
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "glob",
            "description": "List workspace files whose path or name matches a glob pattern (supports **).",
            "parameters": {
                "type": "object",
                "properties": {"pattern": {"type": "string", "description": "Glob pattern, e.g. src/**/*.rs"}},
                "required": ["pattern"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "grep",
            "description": "Search workspace text files with a regular expression; returns matching lines with paths.",
            "parameters": {
                "type": "object",
                "properties": {"pattern": {"type": "string", "description": "Rust regex pattern"}},
                "required": ["pattern"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "shell",
            "description": "Run a shell command in the workspace (PowerShell 7 on Windows, bash elsewhere). Long commands time out.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_ms": {"type": "integer", "description": "Optional timeout in milliseconds (default 30000, max 120000)"}
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }),
    ];
    definitions.extend(crate::db::db_tool_definitions());
    definitions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_fill_tool_budget() {
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.len(), MAX_TOOLS);
        assert_eq!(
            registry
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            [
                "read_file",
                "write_file",
                "edit_file",
                "glob",
                "grep",
                "shell",
                "db_list",
                "db_schema",
                "db_query",
                "db_explain",
                "db_execute",
                "mcp_tool",
            ]
        );
        assert_eq!(registry.get("shell").map(|s| s.risk), Some(Risk::Execute));
        assert_eq!(
            registry.get("write_file").map(|s| s.risk),
            Some(Risk::Write)
        );
        for name in [
            "db_list",
            "db_schema",
            "db_query",
            "db_explain",
            "db_execute",
        ] {
            assert_eq!(
                registry.get(name).map(|spec| spec.risk),
                Some(Risk::Network),
                "{name} 必须是 Network"
            );
        }
        for removed in ["search_code", "todo_write", "ssh_exec", "task"] {
            assert!(registry.get(removed).is_none(), "{removed} 已移出预算");
        }
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

    #[test]
    fn builtin_definitions_cover_executor_tools() {
        let registry = ToolRegistry::with_builtins();
        let tools = registry.llm_tools();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "read_file",
                "write_file",
                "edit_file",
                "glob",
                "grep",
                "shell",
                "db_list",
                "db_schema",
                "db_query",
                "db_explain",
                "db_execute",
            ]
        );
        for tool in &tools {
            assert_eq!(tool["type"], "function");
            assert_eq!(tool["parameters"]["type"], "object");
            assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
        }
    }

    #[test]
    fn llm_tools_include_database_schemas() {
        let tools = ToolRegistry::with_builtins().llm_tools();
        for name in [
            "db_list",
            "db_schema",
            "db_query",
            "db_explain",
            "db_execute",
        ] {
            let tool = tools
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("缺少 {name} 定义"));
            assert_eq!(tool["parameters"]["type"], "object");
        }
        assert!(
            tools.iter().all(|tool| tool["name"] != "mcp_tool"),
            "mcp_tool 是占位，不应出现在静态定义中"
        );
    }

    #[test]
    fn definitions_are_filtered_by_registry() {
        let mut registry = ToolRegistry::new();
        registry
            .register(ToolSpec::new("read_file", Risk::Read))
            .expect("register");
        let tools = registry.llm_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read_file");
    }

    #[test]
    fn with_mcp_tools_replaces_placeholder_without_exceeding_budget() {
        let registry = ToolRegistry::with_builtins().with_mcp_tools([]);
        assert_eq!(registry.len(), MAX_TOOLS - 1);
        assert!(registry.get("mcp_tool").is_none());
        assert!(registry.get("db_query").is_some());
        assert!(registry.get("db_execute").is_some());
        assert!(registry.get("task").is_none());
    }

    #[test]
    fn with_mcp_tools_caps_dynamic_tools_at_max() {
        let specs =
            (0..5).map(|index| ToolSpec::new(format!("mcp_server_tool_{index}"), Risk::Network));
        let registry = ToolRegistry::with_builtins().with_mcp_tools(specs);

        assert_eq!(registry.len(), MAX_TOOLS);
        assert!(registry.get("mcp_tool").is_none());
        let dynamic: Vec<&str> = registry
            .iter()
            .filter(|spec| spec.name.starts_with("mcp_server_"))
            .map(|spec| spec.name.as_str())
            .collect();
        assert_eq!(dynamic, ["mcp_server_tool_0"]);
        assert_eq!(
            registry.get("mcp_server_tool_0").map(|spec| spec.risk),
            Some(Risk::Network)
        );
    }

    #[test]
    fn with_mcp_tools_skips_duplicates() {
        let existing = ToolRegistry::with_builtins();
        let registry = existing.with_mcp_tools([
            ToolSpec::new("read_file", Risk::Network),
            ToolSpec::new("mcp_ok", Risk::Network),
        ]);
        assert_eq!(
            registry.get("read_file").map(|spec| spec.risk),
            Some(Risk::Read)
        );
        assert!(registry.get("mcp_ok").is_some());
        assert_eq!(registry.len(), MAX_TOOLS);
    }
}
