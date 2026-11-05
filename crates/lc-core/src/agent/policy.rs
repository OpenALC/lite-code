//! 权限策略纯函数（docs/AGENT.md §5）。
//!
//! 三层判定：静态黑名单 → 用户黑名单 → 模型风险（仅 auto）。执行闸门在
//! Rust 侧；UI 只展示 `PolicyDecision` 并收集审批，不能绕过 Allow/Ask/Deny。
//! "本次会话允许同类"的审批缓存属于会话层，不在这里。

use regex::{Regex, RegexSet};
use thiserror::Error;

use super::tools::Risk;

/// 四种策略预设（docs/AGENT.md §5.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyMode {
    Plan,
    Chat,
    Auto,
    Yolo,
}

/// 判定来源，审批卡据此展示"为什么被拦"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicySource {
    Mode,
    StaticBlacklist,
    UserBlacklist,
    ModelRisk,
    OutsideWorkspace,
    NetworkTool,
    ExecuteReview,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Ask {
        source: PolicySource,
        reason: String,
    },
    Deny {
        source: PolicySource,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRequest<'a> {
    pub mode: PolicyMode,
    pub tool: &'a str,
    pub risk: Risk,
    /// shell/SQL 等可匹配黑名单的完整命令原文。
    pub command: Option<&'a str>,
    /// Write 工具是否落在工作区根内。
    pub inside_workspace: bool,
    /// auto 模式的模型风险判定；None = 未判定（按高风险 fail-closed）。
    pub model_risk_high: Option<bool>,
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("用户黑名单正则无效（{pattern}）：{source}")]
    InvalidUserPattern {
        pattern: String,
        #[source]
        source: regex::Error,
    },
}

/// 静态黑名单：硬编码、任何模式都不能绕过（命中即 Ask）。
const STATIC_PATTERNS: &[&str] = &[
    r"(?i)\brm\s+-rf\s+(/|~)",
    r"(?i)\bmkfs(\.|\s)",
    r"(?i)\bdd\s+.*of=/dev/",
    r"(?i)\bgit\s+push\b.*--force",
    r"(?i)\bgit\s+reset\s+--hard",
    r"(?i)\bgit\s+clean\s+-[a-z]*f[a-z]*x",
    r"(?i)\bdrop\s+database\b",
    r"(?i)\btruncate\s+",
    r"(?i)\bshutdown\s+/s\b",
    r"(?i)\breg\s+delete\b",
    r"(?i)\bformat\s+[a-z]:",
    r"(?i)remove-item\b.*-recurse\b.*-force\b.*c:\\",
    r"(?i)\b(curl|iwr|wget)\b.*\|\s*(sh|bash|iex)\b",
    r"(?i)\bchmod\s+777\s+/",
];

pub struct Policy {
    static_blacklist: RegexSet,
    user_blacklist: Vec<Regex>,
}

impl Policy {
    /// 用户黑名单按正则编译；任一无效直接报错，避免静默失效。
    pub fn new(user_patterns: &[String]) -> Result<Self, PolicyError> {
        let static_blacklist = RegexSet::new(STATIC_PATTERNS).expect("静态黑名单正则必须合法");
        let user_blacklist = user_patterns
            .iter()
            .map(|pattern| {
                Regex::new(pattern).map_err(|source| PolicyError::InvalidUserPattern {
                    pattern: pattern.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            static_blacklist,
            user_blacklist,
        })
    }

    /// 无用户黑名单的默认策略。
    pub fn with_defaults() -> Self {
        Self::new(&[]).expect("默认策略无用户规则")
    }

    pub fn evaluate(&self, request: &PolicyRequest<'_>) -> PolicyDecision {
        if request.tool.is_empty() {
            return PolicyDecision::Deny {
                source: PolicySource::Mode,
                reason: "工具名为空".into(),
            };
        }

        match request.mode {
            PolicyMode::Chat => PolicyDecision::Deny {
                source: PolicySource::Mode,
                reason: "仅对话模式不调用工具".into(),
            },
            PolicyMode::Plan => {
                if request.risk == Risk::Read {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Deny {
                        source: PolicySource::Mode,
                        reason: "计划模式只允许只读工具".into(),
                    }
                }
            }
            PolicyMode::Auto | PolicyMode::Yolo => self.evaluate_autonomous(request),
        }
    }

    fn evaluate_autonomous(&self, request: &PolicyRequest<'_>) -> PolicyDecision {
        if let Some(command) = request.command {
            if let Some(index) = self.static_blacklist.matches(command).iter().next() {
                return PolicyDecision::Ask {
                    source: PolicySource::StaticBlacklist,
                    reason: format!("静态黑名单命中：{}", STATIC_PATTERNS[index]),
                };
            }
            if let Some(pattern) = self
                .user_blacklist
                .iter()
                .find(|pattern| pattern.is_match(command))
            {
                return PolicyDecision::Ask {
                    source: PolicySource::UserBlacklist,
                    reason: format!("用户黑名单命中：{}", pattern.as_str()),
                };
            }
        }

        if request.mode == PolicyMode::Yolo {
            return PolicyDecision::Allow;
        }

        match request.risk {
            Risk::Read => PolicyDecision::Allow,
            Risk::Write => {
                if request.inside_workspace {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Ask {
                        source: PolicySource::OutsideWorkspace,
                        reason: "写入目标在工作区之外".into(),
                    }
                }
            }
            Risk::Execute => match request.model_risk_high {
                Some(false) => PolicyDecision::Allow,
                Some(true) => PolicyDecision::Ask {
                    source: PolicySource::ModelRisk,
                    reason: "模型判定为高风险".into(),
                },
                None => PolicyDecision::Ask {
                    source: PolicySource::ExecuteReview,
                    reason: "执行类工具未取得模型风险判定（按高风险处理）".into(),
                },
            },
            Risk::Network => PolicyDecision::Ask {
                source: PolicySource::NetworkTool,
                reason: "网络/外部工具需要确认".into(),
            },
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(mode: PolicyMode, risk: Risk) -> PolicyRequest<'a> {
        PolicyRequest {
            mode,
            tool: "t",
            risk,
            command: None,
            inside_workspace: true,
            model_risk_high: None,
        }
    }

    #[test]
    fn plan_allows_read_and_denies_writes() {
        let policy = Policy::default();
        assert_eq!(
            policy.evaluate(&request(PolicyMode::Plan, Risk::Read)),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy.evaluate(&request(PolicyMode::Plan, Risk::Write)),
            PolicyDecision::Deny {
                source: PolicySource::Mode,
                ..
            }
        ));
    }

    #[test]
    fn chat_denies_all_tools() {
        let policy = Policy::default();
        for risk in [Risk::Read, Risk::Write, Risk::Execute, Risk::Network] {
            assert!(matches!(
                policy.evaluate(&request(PolicyMode::Chat, risk)),
                PolicyDecision::Deny { .. }
            ));
        }
    }

    #[test]
    fn static_blacklist_asks_even_in_yolo() {
        let policy = Policy::default();
        let mut req = request(PolicyMode::Yolo, Risk::Execute);
        req.command = Some("git push --force origin main");
        assert!(matches!(
            policy.evaluate(&req),
            PolicyDecision::Ask {
                source: PolicySource::StaticBlacklist,
                ..
            }
        ));

        let mut req = request(PolicyMode::Yolo, Risk::Execute);
        req.command = Some("rm -rf /");
        assert!(matches!(
            policy.evaluate(&req),
            PolicyDecision::Ask {
                source: PolicySource::StaticBlacklist,
                ..
            }
        ));
    }

    #[test]
    fn user_blacklist_asks_in_auto_and_yolo() {
        let policy = Policy::new(&[r"(?i)\bnpm\s+publish\b".into()]).expect("pattern");
        for mode in [PolicyMode::Auto, PolicyMode::Yolo] {
            let mut req = request(mode, Risk::Execute);
            req.command = Some("npm publish --access public");
            assert!(matches!(
                policy.evaluate(&req),
                PolicyDecision::Ask {
                    source: PolicySource::UserBlacklist,
                    ..
                }
            ));
        }
    }

    #[test]
    fn auto_write_inside_is_allowed_outside_asks() {
        let policy = Policy::default();
        assert_eq!(
            policy.evaluate(&request(PolicyMode::Auto, Risk::Write)),
            PolicyDecision::Allow
        );
        let mut req = request(PolicyMode::Auto, Risk::Write);
        req.inside_workspace = false;
        assert!(matches!(
            policy.evaluate(&req),
            PolicyDecision::Ask {
                source: PolicySource::OutsideWorkspace,
                ..
            }
        ));
    }

    #[test]
    fn auto_execute_follows_model_risk_fail_closed() {
        let policy = Policy::default();
        let mut req = request(PolicyMode::Auto, Risk::Execute);
        req.model_risk_high = Some(false);
        assert_eq!(policy.evaluate(&req), PolicyDecision::Allow);

        req.model_risk_high = Some(true);
        assert!(matches!(
            policy.evaluate(&req),
            PolicyDecision::Ask {
                source: PolicySource::ModelRisk,
                ..
            }
        ));

        req.model_risk_high = None;
        assert!(matches!(
            policy.evaluate(&req),
            PolicyDecision::Ask {
                source: PolicySource::ExecuteReview,
                ..
            }
        ));
    }

    #[test]
    fn auto_network_asks_yolo_allows_without_blacklist() {
        let policy = Policy::default();
        assert!(matches!(
            policy.evaluate(&request(PolicyMode::Auto, Risk::Network)),
            PolicyDecision::Ask {
                source: PolicySource::NetworkTool,
                ..
            }
        ));
        assert_eq!(
            policy.evaluate(&request(PolicyMode::Yolo, Risk::Network)),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn invalid_user_pattern_is_reported() {
        assert!(matches!(
            Policy::new(&["(".into()]),
            Err(PolicyError::InvalidUserPattern { .. })
        ));
    }
}
