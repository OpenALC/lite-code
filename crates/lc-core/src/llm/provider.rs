//! 供应商与模型配置（docs/AGENT.md §3.1）。密钥不在这里：只存 `apiKeyRef`。

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    /// v1 端点；OpenAI 兼容服务通过 baseUrl 覆盖。
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// v1.x 端点：Anthropic Messages API（docs/AGENT.md §3）。
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    pub base_url: String,
    pub api_key_ref: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    pub provider: String,
    pub model_id: String,
    #[serde(default)]
    pub thinking: Vec<String>,
    pub context_k: u32,
    pub compress_at_k: u32,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersFile {
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderError {
    #[error("配置解析失败：{0}")]
    Parse(String),
    #[error("provider id 重复：{0}")]
    DuplicateProvider(String),
    #[error("model id 重复：{0}")]
    DuplicateModel(String),
    #[error("model {model} 引用了不存在的 provider {provider}")]
    UnknownProvider { model: String, provider: String },
    #[error("model {0} 的 compressAtK 必须大于 0 且小于 contextK")]
    InvalidBudget(String),
    #[error("defaultModel {0} 不存在")]
    UnknownDefaultModel(String),
    #[error("配置缺少 provider 或 model")]
    Empty,
}

impl ProvidersFile {
    pub fn parse(text: &str) -> Result<Self, ProviderError> {
        serde_json::from_str(text).map_err(|e| ProviderError::Parse(e.to_string()))
    }

    pub fn serialize(&self) -> String {
        serde_json::to_string_pretty(self).expect("ProvidersFile 序列化不可能失败")
    }

    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.providers.is_empty() || self.models.is_empty() {
            return Err(ProviderError::Empty);
        }
        let mut provider_ids = std::collections::HashSet::new();
        for provider in &self.providers {
            if !provider_ids.insert(provider.id.as_str()) {
                return Err(ProviderError::DuplicateProvider(provider.id.clone()));
            }
        }
        let mut model_ids = std::collections::HashSet::new();
        for model in &self.models {
            if !model_ids.insert(model.id.as_str()) {
                return Err(ProviderError::DuplicateModel(model.id.clone()));
            }
            if !provider_ids.contains(model.provider.as_str()) {
                return Err(ProviderError::UnknownProvider {
                    model: model.id.clone(),
                    provider: model.provider.clone(),
                });
            }
            if model.compress_at_k == 0 || model.compress_at_k >= model.context_k {
                return Err(ProviderError::InvalidBudget(model.id.clone()));
            }
        }
        if let Some(default) = &self.default_model
            && !model_ids.contains(default.as_str())
        {
            return Err(ProviderError::UnknownDefaultModel(default.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
      "providers": [
        {"id":"openai","kind":"openai-responses","label":"OpenAI","baseUrl":"https://api.openai.com/v1","apiKeyRef":"keyring://openai","enabled":true}
      ],
      "models": [
        {"id":"gpt-5","provider":"openai","modelId":"gpt-5","thinking":["off","low"],"contextK":400,"compressAtK":300,"capabilities":["tools"]}
      ],
      "defaultModel": "gpt-5",
      "defaultThinking": "low"
    }"#;

    #[test]
    fn parses_and_serializes_camel_case_config() {
        let file = ProvidersFile::parse(VALID).expect("parse");
        file.validate().expect("valid");
        assert_eq!(file.providers[0].base_url, "https://api.openai.com/v1");
        assert_eq!(file.models[0].context_k, 400);
        let roundtrip = ProvidersFile::parse(&file.serialize()).expect("roundtrip");
        assert_eq!(roundtrip, file);
    }

    #[test]
    fn rejects_duplicates_unknown_provider_and_bad_budget() {
        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.providers.push(file.providers[0].clone());
        assert!(matches!(
            file.validate(),
            Err(ProviderError::DuplicateProvider(_))
        ));

        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.models[0].provider = "missing".into();
        assert!(matches!(
            file.validate(),
            Err(ProviderError::UnknownProvider { .. })
        ));

        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.models[0].compress_at_k = 400;
        assert!(matches!(
            file.validate(),
            Err(ProviderError::InvalidBudget(_))
        ));
    }

    #[test]
    fn unknown_default_model_is_rejected() {
        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.default_model = Some("ghost".into());
        assert!(matches!(
            file.validate(),
            Err(ProviderError::UnknownDefaultModel(_))
        ));
    }

    #[test]
    fn provider_kind_roundtrips_both_serde_names() {
        let openai = serde_json::to_string(&ProviderKind::OpenAiResponses).unwrap();
        assert_eq!(openai, r#""openai-responses""#);
        assert_eq!(
            serde_json::from_str::<ProviderKind>(&openai).unwrap(),
            ProviderKind::OpenAiResponses
        );

        let anthropic = serde_json::to_string(&ProviderKind::AnthropicMessages).unwrap();
        assert_eq!(anthropic, r#""anthropic-messages""#);
        assert_eq!(
            serde_json::from_str::<ProviderKind>(&anthropic).unwrap(),
            ProviderKind::AnthropicMessages
        );
    }

    #[test]
    fn parses_mixed_provider_config_without_breaking_existing() {
        const MIXED: &str = r#"{
          "providers": [
            {"id":"openai","kind":"openai-responses","label":"OpenAI","baseUrl":"https://api.openai.com/v1","apiKeyRef":"keyring://openai","enabled":true},
            {"id":"anthropic","kind":"anthropic-messages","label":"Anthropic","baseUrl":"https://api.anthropic.com/v1","apiKeyRef":"keyring://anthropic","enabled":true}
          ],
          "models": [
            {"id":"gpt-5","provider":"openai","modelId":"gpt-5","thinking":["off","low"],"contextK":400,"compressAtK":300,"capabilities":["tools"]},
            {"id":"claude","provider":"anthropic","modelId":"claude-sonnet-4-5","thinking":["off","low","high"],"contextK":200,"compressAtK":150,"capabilities":["tools"]}
          ],
          "defaultModel": "claude",
          "defaultThinking": "low"
        }"#;
        let file = ProvidersFile::parse(MIXED).expect("parse");
        file.validate().expect("valid");
        assert_eq!(file.providers[1].kind, ProviderKind::AnthropicMessages);
        assert_eq!(file.models[1].provider, "anthropic");
        let roundtrip = ProvidersFile::parse(&file.serialize()).expect("roundtrip");
        assert_eq!(roundtrip, file);
    }
}
