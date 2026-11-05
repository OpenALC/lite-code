//! 供应商运行配置（docs/AGENT.md §3.1）：`providers.json` → 可用的
//! endpoint + 模型 + 密钥。SecretStore 落地前，密钥引用支持 `env:NAME`，
//! 未命中时回退 `LC_API_KEY`；配置文件缺失时完全走环境变量。
//!
//! 配置位置默认为 `~/.lite-code/providers.json`（与 config.json 同目录），
//! 测试/便携场景可用 `LC_PROVIDERS_FILE` 指向别处。

use std::path::{Path, PathBuf};

use super::client::{OpenAiResponsesClient, UreqSseTransport};
use super::provider::{ProviderError, ProviderKind, ProvidersFile};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-5";
pub const DEFAULT_CONTEXT_K: u32 = 400;
pub const DEFAULT_COMPRESS_AT_K: u32 = 300;

/// 一次 Agent 会话需要的供应商侧配置。`model_id` 是发请求的实际 id，
/// `model_alias` 是 UI/配置里的显示 id（如 `gpt-5`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntime {
    pub provider_id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub model_alias: String,
    pub model_id: String,
    pub context_k: u32,
    pub compress_at_k: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("读取 providers 配置失败：{0}")]
    Io(String),
    #[error(transparent)]
    Config(#[from] ProviderError),
    #[error("providers 配置没有可用模型")]
    NoModel,
    #[error("provider {0} 不存在或未启用")]
    ProviderUnavailable(String),
    #[error(
        "无法解析密钥引用 {0}：SecretStore 尚未实现，请改用 env:VAR，或设置环境变量 LC_API_KEY"
    )]
    UnresolvedKeyRef(String),
    #[error("环境变量 {0} 未设置")]
    MissingEnv(String),
    #[error("endpoint {0:?} 的流式客户端尚未实现（P3 仅支持 openai-responses）")]
    UnsupportedKind(ProviderKind),
}

impl AgentRuntime {
    /// OpenAI Responses 流式客户端。Anthropic 已有事件解码器，但传输层
    /// 尚未实现，这里显式 fail-closed。
    pub fn openai_client(&self) -> Result<OpenAiResponsesClient, RuntimeError> {
        match self.kind {
            ProviderKind::OpenAiResponses => Ok(OpenAiResponsesClient::new(
                &self.base_url,
                &self.api_key,
                Box::new(UreqSseTransport::default()),
            )),
            ProviderKind::AnthropicMessages => Err(RuntimeError::UnsupportedKind(self.kind)),
        }
    }
}

/// `~/.lite-code/providers.json`，可用 `LC_PROVIDERS_FILE` 覆盖。
pub fn providers_path() -> Result<PathBuf, RuntimeError> {
    if let Some(path) = std::env::var_os("LC_PROVIDERS_FILE") {
        return Ok(PathBuf::from(path));
    }
    let mut path = crate::config::config_path().map_err(|e| RuntimeError::Io(e.to_string()))?;
    path.set_file_name("providers.json");
    Ok(path)
}

/// 默认加载：读配置文件；缺失则环境变量兜底。
pub fn load_default() -> Result<AgentRuntime, RuntimeError> {
    load_from(&providers_path()?, |name| std::env::var(name).ok())
}

/// 从指定路径加载，并注入环境变量查找函数（测试可完全离线）。
///
/// - 文件存在：解析 + 校验，取 `defaultModel`（缺省取第一个模型）；
///   密钥优先 `LC_API_KEY`，否则按 `apiKeyRef`（仅 `env:NAME`）。
/// - 文件不存在：`LC_BASE_URL` / `LC_MODEL` / `LC_API_KEY`，缺密钥报错。
pub fn load_from(
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Result<AgentRuntime, RuntimeError> {
    match std::fs::read_to_string(path) {
        Ok(text) => runtime_from_str(&text, env),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fallback_from_env(env),
        Err(e) => Err(RuntimeError::Io(e.to_string())),
    }
}

fn runtime_from_str(
    text: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<AgentRuntime, RuntimeError> {
    let file = ProvidersFile::parse(text)?;
    file.validate()?;

    let model = file
        .default_model
        .as_deref()
        .and_then(|id| file.models.iter().find(|model| model.id == id))
        .or_else(|| file.models.first())
        .ok_or(RuntimeError::NoModel)?;
    let provider = file
        .providers
        .iter()
        .find(|provider| provider.id == model.provider && provider.enabled)
        .ok_or_else(|| RuntimeError::ProviderUnavailable(model.provider.clone()))?;

    let api_key = match non_empty(env("LC_API_KEY")) {
        Some(key) => key,
        None => resolve_key_ref(&provider.api_key_ref, env)?,
    };

    Ok(AgentRuntime {
        provider_id: provider.id.clone(),
        kind: provider.kind,
        base_url: provider.base_url.clone(),
        api_key,
        model_alias: model.id.clone(),
        model_id: model.model_id.clone(),
        context_k: model.context_k,
        compress_at_k: model.compress_at_k,
    })
}

fn resolve_key_ref(
    reference: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, RuntimeError> {
    if let Some(name) = reference.strip_prefix("env:") {
        return non_empty(env(name)).ok_or_else(|| RuntimeError::MissingEnv(name.into()));
    }
    Err(RuntimeError::UnresolvedKeyRef(reference.into()))
}

fn fallback_from_env(env: impl Fn(&str) -> Option<String>) -> Result<AgentRuntime, RuntimeError> {
    let api_key = non_empty(env("LC_API_KEY"))
        .or_else(|| non_empty(env("OPENAI_API_KEY")))
        .ok_or_else(|| RuntimeError::MissingEnv("LC_API_KEY".into()))?;
    Ok(AgentRuntime {
        provider_id: "env".into(),
        kind: ProviderKind::OpenAiResponses,
        base_url: non_empty(env("LC_BASE_URL")).unwrap_or_else(|| DEFAULT_BASE_URL.into()),
        api_key,
        model_alias: DEFAULT_MODEL.into(),
        model_id: non_empty(env("LC_MODEL")).unwrap_or_else(|| DEFAULT_MODEL.into()),
        context_k: parse_k(env("LC_CONTEXT_K"), DEFAULT_CONTEXT_K),
        compress_at_k: parse_k(env("LC_COMPRESS_AT_K"), DEFAULT_COMPRESS_AT_K),
    })
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

fn parse_k(value: Option<String>, fallback: u32) -> u32 {
    value
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    const CONFIG: &str = r#"{
      "providers": [
        {"id":"openai","kind":"openai-responses","label":"OpenAI","baseUrl":"https://example.test/v1","apiKeyRef":"env:TEST_OPENAI_KEY","enabled":true},
        {"id":"anthropic","kind":"anthropic-messages","label":"Anthropic","baseUrl":"https://api.anthropic.com/v1","apiKeyRef":"env:TEST_ANTHROPIC_KEY","enabled":true}
      ],
      "models": [
        {"id":"gpt-5","provider":"openai","modelId":"gpt-5-real","thinking":["off"],"contextK":400,"compressAtK":300,"capabilities":["tools"]},
        {"id":"claude","provider":"anthropic","modelId":"claude-real","thinking":["off"],"contextK":200,"compressAtK":150,"capabilities":["tools"]}
      ],
      "defaultModel": "gpt-5",
      "defaultThinking": "off"
    }"#;

    fn temp_file(tag: &str, contents: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lc-provider-runtime-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn loads_default_model_and_env_key() {
        let path = temp_file("ok", CONFIG);
        let rt = load_from(&path, env_of(&[("TEST_OPENAI_KEY", "sk-test")])).expect("load");
        assert_eq!(rt.kind, ProviderKind::OpenAiResponses);
        assert_eq!(rt.base_url, "https://example.test/v1");
        assert_eq!(rt.model_alias, "gpt-5");
        assert_eq!(rt.model_id, "gpt-5-real");
        assert_eq!(rt.api_key, "sk-test");
        assert_eq!(rt.context_k, 400);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn lc_api_key_overrides_reference() {
        let path = temp_file("override", CONFIG);
        let rt = load_from(
            &path,
            env_of(&[("LC_API_KEY", "global-key"), ("TEST_OPENAI_KEY", "ignored")]),
        )
        .expect("load");
        assert_eq!(rt.api_key, "global-key");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_file_falls_back_to_environment() {
        let missing = std::env::temp_dir().join("lc-provider-runtime-no-such-file.json");
        let rt = load_from(
            &missing,
            env_of(&[
                ("LC_API_KEY", "sk-env"),
                ("LC_MODEL", "my-model"),
                ("LC_BASE_URL", "http://127.0.0.1:9/v1"),
                ("LC_CONTEXT_K", "128"),
            ]),
        )
        .expect("fallback");
        assert_eq!(rt.api_key, "sk-env");
        assert_eq!(rt.model_id, "my-model");
        assert_eq!(rt.base_url, "http://127.0.0.1:9/v1");
        assert_eq!(rt.context_k, 128);
        assert_eq!(rt.compress_at_k, DEFAULT_COMPRESS_AT_K);
    }

    #[test]
    fn missing_key_reports_env_name() {
        let path = temp_file("nokey", CONFIG);
        let err = load_from(&path, env_of(&[])).expect_err("no key");
        assert_eq!(err, RuntimeError::MissingEnv("TEST_OPENAI_KEY".into()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn keyring_ref_is_not_silently_ignored() {
        let config = CONFIG.replace("env:TEST_OPENAI_KEY", "keyring://lite-code/openai");
        let path = temp_file("keyring", &config);
        let err = load_from(&path, env_of(&[])).expect_err("keyring");
        assert!(matches!(err, RuntimeError::UnresolvedKeyRef(_)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn anthropic_runtime_is_rejected_at_client_build() {
        let config = CONFIG.replace(r#""defaultModel": "gpt-5""#, r#""defaultModel": "claude""#);
        let path = temp_file("anthropic", &config);
        let rt = load_from(&path, env_of(&[("TEST_ANTHROPIC_KEY", "sk-ant")])).expect("load");
        assert_eq!(
            rt.openai_client().err(),
            Some(RuntimeError::UnsupportedKind(
                ProviderKind::AnthropicMessages
            ))
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn invalid_config_is_reported() {
        let path = temp_file("invalid", r#"{"providers":[],"models":[]}"#);
        let err = load_from(&path, env_of(&[])).expect_err("invalid");
        assert!(matches!(err, RuntimeError::Config(ProviderError::Empty)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
