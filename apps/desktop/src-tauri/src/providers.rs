//! 供应商管理命令层：读取/保存 `~/.lite-code/providers.json`。
//!
//! 视图构造与保存合并语义在 lc-core `llm::providers_view`（桌面与鸿蒙宿主共用）；
//! 本模块只负责路径解析、命令注册与错误码映射。
//!
//! 返回给前端的供应商永远脱敏（`hasApiKey` + 末四位提示），API Key 不回传；
//! 保存时空白 Key 表示保留旧值，避免用户只改 Base URL 就把密钥清掉。

use lc_core::llm::runtime::{RuntimeError, providers_path};
use lc_core::llm::{
    ProviderError, ProvidersFile, ProvidersSaveInput, ProvidersView, merge_and_validate,
    view_from_file,
};
use lc_proto::{ErrorBody, ErrorCode};

#[tauri::command]
pub fn providers_load() -> Result<ProvidersView, ErrorBody> {
    let path = providers_path().map_err(runtime_error)?;
    let file = ProvidersFile::load_path(&path).map_err(provider_error)?;
    Ok(view_from_file(file.unwrap_or_default()))
}

#[tauri::command]
pub fn providers_save(input: ProvidersSaveInput) -> Result<ProvidersView, ErrorBody> {
    let path = providers_path().map_err(runtime_error)?;
    let existing = ProvidersFile::load_path(&path).map_err(provider_error)?;
    let file = merge_and_validate(existing, input).map_err(provider_error)?;
    file.save_path(&path).map_err(provider_error)?;
    Ok(view_from_file(file))
}

fn provider_error(error: ProviderError) -> ErrorBody {
    let code = match error {
        ProviderError::Io(_) => ErrorCode::Io,
        _ => ErrorCode::InvalidArgument,
    };
    ErrorBody::new(code, error.to_string())
}

fn runtime_error(error: RuntimeError) -> ErrorBody {
    let code = match error {
        RuntimeError::Io(_) => ErrorCode::Io,
        _ => ErrorCode::Internal,
    };
    ErrorBody::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lc_core::llm::{ModelConfig, ProviderConfig, ProviderInput, ProviderKind};

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

    fn input(key: Option<&str>, clear: bool) -> ProvidersSaveInput {
        ProvidersSaveInput {
            providers: vec![ProviderInput {
                id: "openai".into(),
                kind: ProviderKind::OpenAiResponses,
                label: "OpenAI".into(),
                base_url: "https://api.openai.com/v1".into(),
                enabled: true,
                api_key: key.map(str::to_string),
                api_key_ref: None,
                clear_api_key: clear,
            }],
            models: vec![model()],
            default_model: Some("gpt-5".into()),
            default_thinking: Some("off".into()),
            subagent_model: Some("gpt-5".into()),
            subagent_thinking: Some("high".into()),
        }
    }

    fn existing(key: Option<&str>) -> ProvidersFile {
        ProvidersFile {
            providers: vec![ProviderConfig {
                id: "openai".into(),
                kind: ProviderKind::OpenAiResponses,
                label: "OpenAI".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: key.map(str::to_string),
                api_key_ref: "env:OPENAI_API_KEY".into(),
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
        let merged =
            merge_and_validate(Some(existing(Some("sk-old"))), input(None, false)).unwrap();
        assert_eq!(merged.providers[0].api_key.as_deref(), Some("sk-old"));

        let merged =
            merge_and_validate(Some(existing(Some("sk-old"))), input(Some("sk-new"), false))
                .unwrap();
        assert_eq!(merged.providers[0].api_key.as_deref(), Some("sk-new"));
    }

    #[test]
    fn clear_key_removes_secret() {
        let merged = merge_and_validate(Some(existing(Some("sk-old"))), input(None, true)).unwrap();
        assert_eq!(merged.providers[0].api_key, None);
        assert_eq!(merged.providers[0].api_key_ref, "");
    }

    #[test]
    fn view_never_exposes_full_key() {
        let view = view_from_file(existing(Some("sk-123456789")));
        assert!(view.providers[0].has_api_key);
        assert_eq!(view.providers[0].api_key_hint.as_deref(), Some("••••6789"));

        let short = view_from_file(existing(Some("abcde")));
        assert_eq!(short.providers[0].api_key_hint.as_deref(), Some("••••"));
    }

    #[test]
    fn invalid_input_surfaces_validation_error() {
        let mut bad = input(None, false);
        bad.models[0].compress_at_k = 500;
        assert!(matches!(
            merge_and_validate(None, bad),
            Err(ProviderError::InvalidBudget(_))
        ));
    }

    #[test]
    fn provider_errors_map_to_contract_codes() {
        assert_eq!(
            provider_error(ProviderError::Io("boom".into())).code,
            ErrorCode::Io
        );
        assert_eq!(
            provider_error(ProviderError::Empty).code,
            ErrorCode::InvalidArgument
        );
    }
}
