//! 供应商管理视图与保存合并（桌面壳与鸿蒙宿主共用）。
//!
//! 返回给前端的供应商永远脱敏（`hasApiKey` + 末四位提示），API Key 不回传；
//! 保存时空白 Key 表示保留旧值，避免用户只改 Base URL 就把密钥清掉。
//! 实际配置语义与校验在 [`super::provider`]，这里只做 DTO 转换与合并。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::provider::{ModelConfig, ProviderConfig, ProviderError, ProviderKind, ProvidersFile};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    pub base_url: String,
    pub enabled: bool,
    pub api_key_ref: String,
    pub has_api_key: bool,
    /// 末四位提示（不返回完整密钥）；无本地密钥时为 None。
    pub api_key_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersView {
    pub providers: Vec<ProviderView>,
    pub models: Vec<ModelConfig>,
    pub default_model: Option<String>,
    pub default_thinking: Option<String>,
    pub subagent_model: Option<String>,
    pub subagent_thinking: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    pub base_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 用户新输入的密钥；空/缺省表示保留旧值。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 旧配置的环境变量引用；缺省表示沿用旧值。
    #[serde(default)]
    pub api_key_ref: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersSaveInput {
    pub providers: Vec<ProviderInput>,
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub default_thinking: Option<String>,
    #[serde(default)]
    pub subagent_model: Option<String>,
    #[serde(default)]
    pub subagent_thinking: Option<String>,
}

/// 把持久化配置转成前端视图：密钥只保留 `hasApiKey` 与末四位提示。
pub fn view_from_file(file: ProvidersFile) -> ProvidersView {
    ProvidersView {
        providers: file
            .providers
            .into_iter()
            .map(|provider| {
                let key = provider
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|key| !key.is_empty());
                ProviderView {
                    id: provider.id,
                    kind: provider.kind,
                    label: provider.label,
                    base_url: provider.base_url,
                    enabled: provider.enabled,
                    api_key_ref: provider.api_key_ref,
                    has_api_key: key.is_some(),
                    api_key_hint: key.map(key_hint),
                }
            })
            .collect(),
        models: file.models,
        default_model: file.default_model,
        default_thinking: file.default_thinking,
        subagent_model: file.subagent_model,
        subagent_thinking: file.subagent_thinking,
    }
}

impl From<ProvidersFile> for ProvidersView {
    fn from(file: ProvidersFile) -> Self {
        view_from_file(file)
    }
}

/// 合并旧配置里的密钥/引用，再走一次校验。纯函数，两端命令层直接复用。
pub fn merge_and_validate(
    existing: Option<ProvidersFile>,
    input: ProvidersSaveInput,
) -> Result<ProvidersFile, ProviderError> {
    let mut old_providers: HashMap<String, ProviderConfig> = existing
        .unwrap_or_default()
        .providers
        .into_iter()
        .map(|provider| (provider.id.clone(), provider))
        .collect();

    let providers = input
        .providers
        .into_iter()
        .map(|provider| {
            let old = old_providers.remove(&provider.id);
            let new_key = provider
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string);
            let api_key = if provider.clear_api_key {
                None
            } else {
                new_key.or_else(|| old.as_ref().and_then(|p| p.api_key.clone()))
            };
            let api_key_ref = if provider.clear_api_key {
                // 清除密钥时一并清掉环境变量引用，避免运行时继续回退到旧密钥。
                String::new()
            } else {
                provider
                    .api_key_ref
                    .as_deref()
                    .map(str::trim)
                    .filter(|reference| !reference.is_empty())
                    .map(str::to_string)
                    .or_else(|| old.map(|p| p.api_key_ref))
                    .unwrap_or_default()
            };
            ProviderConfig {
                id: provider.id,
                kind: provider.kind,
                label: provider.label,
                base_url: provider.base_url,
                api_key,
                api_key_ref,
                enabled: provider.enabled,
            }
        })
        .collect();

    let file = ProvidersFile {
        providers,
        models: input.models,
        default_model: input.default_model,
        default_thinking: input.default_thinking,
        subagent_model: input.subagent_model,
        subagent_thinking: input.subagent_thinking,
    };
    file.validate()?;
    Ok(file)
}

/// 只暴露末四位；短密钥整体打码，避免把 4 位以内的密钥直接回显。
fn key_hint(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 8 {
        return "••••".into();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("••••{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelConfig {
        ModelConfig {
            id: "gpt-5".into(),
            provider: "openai".into(),
            model_id: "gpt-5".into(),
            thinking: vec!["off".into(), "high".into()],
            context_k: 400,
            compress_at_k: 300,
            capabilities: vec!["tools".into()],
        }
    }

    fn input(
        id: &str,
        key: Option<&str>,
        reference: Option<&str>,
        clear: bool,
    ) -> ProvidersSaveInput {
        ProvidersSaveInput {
            providers: vec![ProviderInput {
                id: id.into(),
                kind: ProviderKind::OpenAiResponses,
                label: "OpenAI".into(),
                base_url: "https://api.openai.com/v1".into(),
                enabled: true,
                api_key: key.map(str::to_string),
                api_key_ref: reference.map(str::to_string),
                clear_api_key: clear,
            }],
            models: vec![model()],
            default_model: Some("gpt-5".into()),
            default_thinking: Some("off".into()),
            subagent_model: Some("gpt-5".into()),
            subagent_thinking: Some("high".into()),
        }
    }

    fn existing(key: Option<&str>, reference: &str) -> ProvidersFile {
        ProvidersFile {
            providers: vec![ProviderConfig {
                id: "openai".into(),
                kind: ProviderKind::OpenAiResponses,
                label: "OpenAI".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: key.map(str::to_string),
                api_key_ref: reference.into(),
                enabled: true,
            }],
            models: vec![model()],
            default_model: Some("gpt-5".into()),
            default_thinking: None,
            subagent_model: None,
            subagent_thinking: None,
        }
    }

    #[test]
    fn blank_key_keeps_existing_and_explicit_key_replaces() {
        let merged = merge_and_validate(
            Some(existing(Some("sk-old"), "env:OPENAI_API_KEY")),
            input("openai", None, None, false),
        )
        .unwrap();
        assert_eq!(merged.providers[0].api_key.as_deref(), Some("sk-old"));

        let merged = merge_and_validate(
            Some(existing(Some("sk-old"), "env:OPENAI_API_KEY")),
            input("openai", Some("   "), None, false),
        )
        .unwrap();
        assert_eq!(merged.providers[0].api_key.as_deref(), Some("sk-old"));

        let merged = merge_and_validate(
            Some(existing(Some("sk-old"), "env:OPENAI_API_KEY")),
            input("openai", Some("sk-new"), None, false),
        )
        .unwrap();
        assert_eq!(merged.providers[0].api_key.as_deref(), Some("sk-new"));
    }

    #[test]
    fn clear_key_removes_secret_and_reference() {
        let merged = merge_and_validate(
            Some(existing(Some("sk-old"), "env:OPENAI_API_KEY")),
            input("openai", None, None, true),
        )
        .unwrap();
        assert_eq!(merged.providers[0].api_key, None);
        assert_eq!(merged.providers[0].api_key_ref, "");
    }

    #[test]
    fn api_key_ref_overrides_and_falls_back_to_existing() {
        let merged = merge_and_validate(
            Some(existing(Some("sk-old"), "env:OLD_REF")),
            input("openai", None, Some(" env:NEW_REF "), false),
        )
        .unwrap();
        assert_eq!(merged.providers[0].api_key_ref, "env:NEW_REF");

        let merged = merge_and_validate(
            Some(existing(Some("sk-old"), "env:OLD_REF")),
            input("openai", None, Some("   "), false),
        )
        .unwrap();
        assert_eq!(merged.providers[0].api_key_ref, "env:OLD_REF");

        let merged = merge_and_validate(None, input("openai", None, None, false)).unwrap();
        assert_eq!(merged.providers[0].api_key_ref, "");
    }

    #[test]
    fn view_never_exposes_full_key_and_serializes_camel_case() {
        let file = existing(Some("  sk-123456789  "), "");
        let view = view_from_file(file);
        assert!(view.providers[0].has_api_key);
        assert_eq!(view.providers[0].api_key_hint.as_deref(), Some("••••6789"));

        let short = view_from_file(existing(Some("abcde"), ""));
        assert_eq!(short.providers[0].api_key_hint.as_deref(), Some("••••"));

        let blank = view_from_file(existing(Some("   "), "env:OPENAI_API_KEY"));
        assert!(!blank.providers[0].has_api_key);
        assert_eq!(blank.providers[0].api_key_hint, None);

        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["providers"][0]["baseUrl"], "https://api.openai.com/v1");
        assert_eq!(json["providers"][0]["apiKeyRef"], "");
        assert_eq!(json["providers"][0]["hasApiKey"], true);
        assert_eq!(json["providers"][0]["apiKeyHint"], "••••6789");
        assert!(json["providers"][0].get("apiKey").is_none());
        assert_eq!(json["defaultModel"], "gpt-5");
        assert!(json["subagentModel"].is_null());
    }

    #[test]
    fn invalid_budget_is_rejected() {
        let mut bad = input("openai", None, None, false);
        bad.models[0].compress_at_k = 500;
        assert!(matches!(
            merge_and_validate(None, bad),
            Err(ProviderError::InvalidBudget(_))
        ));
    }

    #[test]
    fn merge_preserves_models_and_defaults() {
        let merged = merge_and_validate(
            Some(existing(Some("sk-old"), "env:OLD_REF")),
            input("openai", Some("sk-new"), None, false),
        )
        .unwrap();
        assert_eq!(merged.models[0].id, "gpt-5");
        assert_eq!(merged.default_model.as_deref(), Some("gpt-5"));
        assert_eq!(merged.default_thinking.as_deref(), Some("off"));
        assert_eq!(merged.subagent_model.as_deref(), Some("gpt-5"));
        assert_eq!(merged.subagent_thinking.as_deref(), Some("high"));
    }
}
