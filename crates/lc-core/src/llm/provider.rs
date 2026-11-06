//! 供应商与模型配置（docs/AGENT.md §3.1）。
//!
//! 密钥有两种来源：`apiKey`（供应商管理页填写的本地明文，文件权限 0600）与
//! `apiKeyRef`（`env:NAME`）。运行时优先 `LC_API_KEY` 环境变量，其次 `apiKey`，
//! 最后解析 `apiKeyRef`。两者都不写入日志。

use std::path::Path;

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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    pub base_url: String,
    /** 本地明文密钥（可选）；UI 保存时为空表示保留旧值。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /** 环境变量引用（兼容旧配置）：`env:NAME`。 */
    #[serde(default)]
    pub api_key_ref: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 手写 Debug：`apiKey` 永远不进入日志/panic 输出。
impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("api_key_ref", &self.api_key_ref)
            .field("enabled", &self.enabled)
            .finish()
    }
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// 子 Agent 使用的模型 alias（未来 subagent lane 读取；当前仅持久化）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<String>,
    /// 子 Agent 的思考挡位（同上）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_thinking: Option<String>,
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
    #[error("provider {0} 的 id/label/baseUrl 不能为空，baseUrl 需以 http:// 或 https:// 开头")]
    InvalidProvider(String),
    #[error("model {0} 的 id/provider/modelId 不能为空，contextK 必须大于 0")]
    InvalidModel(String),
    #[error("defaultModel {0} 不存在")]
    UnknownDefaultModel(String),
    #[error("subagentModel {0} 不存在")]
    UnknownSubagentModel(String),
    #[error("配置缺少 provider 或 model")]
    Empty,
    #[error("读写 providers 配置失败：{0}")]
    Io(String),
}

impl ProvidersFile {
    pub fn parse(text: &str) -> Result<Self, ProviderError> {
        serde_json::from_str(text).map_err(|e| ProviderError::Parse(e.to_string()))
    }

    pub fn serialize(&self) -> String {
        serde_json::to_string_pretty(self).expect("ProvidersFile 序列化不可能失败")
    }

    /// 读取配置文件；文件不存在返回 `Ok(None)`（首次配置）。
    pub fn load_path(path: &Path) -> Result<Option<Self>, ProviderError> {
        let file = match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ProviderError::Io(e.to_string())),
        };
        // 旧版本可能以 0644 保存过密钥；加载时收紧并把备份里的密钥抹掉。
        restrict_permissions(path).map_err(|e| ProviderError::Io(e.to_string()))?;
        redact_backup_keys(path);
        restrict_permissions(path).map_err(|e| ProviderError::Io(e.to_string()))?;
        Ok(Some(file))
    }

    /// 校验后原子写入（备份 `.bak`）。Unix 下把文件与备份收紧到 0600：
    /// 供应商页允许直接保存 API Key，不能依赖默认 umask。
    pub fn save_path(&self, path: &Path) -> Result<(), ProviderError> {
        self.validate()?;
        // 先收紧旧文件权限：备份复制会沿用旧文件权限。
        restrict_permissions(path).map_err(|e| ProviderError::Io(e.to_string()))?;
        write_private(path, &self.serialize()).map_err(|e| ProviderError::Io(e.to_string()))?;
        // .bak 只保留非密钥字段：清除/轮换密钥后，旧密钥不能继续留在备份里。
        redact_backup_keys(path);
        restrict_permissions(path).map_err(|e| ProviderError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.providers.is_empty() || self.models.is_empty() {
            return Err(ProviderError::Empty);
        }
        let mut provider_ids = std::collections::HashSet::new();
        for provider in &self.providers {
            if provider.id.trim().is_empty()
                || provider.label.trim().is_empty()
                || !(provider.base_url.starts_with("http://")
                    || provider.base_url.starts_with("https://"))
            {
                return Err(ProviderError::InvalidProvider(provider.id.clone()));
            }
            if !provider_ids.insert(provider.id.as_str()) {
                return Err(ProviderError::DuplicateProvider(provider.id.clone()));
            }
        }
        let mut model_ids = std::collections::HashSet::new();
        for model in &self.models {
            if model.id.trim().is_empty()
                || model.provider.trim().is_empty()
                || model.model_id.trim().is_empty()
                || model.context_k == 0
            {
                return Err(ProviderError::InvalidModel(model.id.clone()));
            }
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
        if let Some(subagent) = &self.subagent_model
            && !model_ids.contains(subagent.as_str())
        {
            return Err(ProviderError::UnknownSubagentModel(subagent.clone()));
        }
        Ok(())
    }
}

/// 备份文件去掉所有 `apiKey`，保留 baseUrl/模型等可恢复信息。
fn redact_backup_keys(path: &Path) {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak = Path::new(&bak);
    let Ok(text) = std::fs::read_to_string(bak) else {
        return;
    };
    let Ok(mut file) = ProvidersFile::parse(&text) else {
        return;
    };
    for provider in &mut file.providers {
        provider.api_key = None;
    }
    let _ = std::fs::write(bak, file.serialize());
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak = Path::new(&bak);
    if bak.exists() {
        std::fs::set_permissions(bak, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// 临时文件创建即 0600，避免 `fs::write` 先按 umask 写明文再 chmod。
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        std::fs::copy(path, Path::new(&bak))?;
    }

    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = Path::new(&tmp);
    let _ = std::fs::remove_file(tmp);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let mut file = std::fs::File::create(tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(tmp, path)
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

    #[test]
    fn parses_local_api_key_and_subagent_fields() {
        const CONFIG: &str = r#"{
          "providers": [
            {"id":"openai","kind":"openai-responses","label":"OpenAI","baseUrl":"https://api.openai.com/v1","apiKey":"sk-local","enabled":true}
          ],
          "models": [
            {"id":"gpt-5","provider":"openai","modelId":"gpt-5","thinking":["off","high"],"contextK":400,"compressAtK":300}
          ],
          "defaultModel":"gpt-5",
          "defaultThinking":"off",
          "subagentModel":"gpt-5",
          "subagentThinking":"high"
        }"#;
        let file = ProvidersFile::parse(CONFIG).expect("parse");
        file.validate().expect("valid");
        assert_eq!(file.providers[0].api_key.as_deref(), Some("sk-local"));
        assert_eq!(file.subagent_model.as_deref(), Some("gpt-5"));
        assert_eq!(file.subagent_thinking.as_deref(), Some("high"));
        let roundtrip = ProvidersFile::parse(&file.serialize()).expect("roundtrip");
        assert_eq!(roundtrip, file);
    }

    #[test]
    fn validates_provider_model_and_subagent_references() {
        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.subagent_model = Some("ghost".into());
        assert!(matches!(
            file.validate(),
            Err(ProviderError::UnknownSubagentModel(_))
        ));

        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.providers[0].base_url = "ftp://example.test".into();
        assert!(matches!(
            file.validate(),
            Err(ProviderError::InvalidProvider(_))
        ));

        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.models[0].context_k = 0;
        assert!(matches!(
            file.validate(),
            Err(ProviderError::InvalidModel(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn saved_files_are_private_and_backup_has_no_key() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "lc-provider-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        let file = ProvidersFile::parse(VALID).expect("parse");
        file.save_path(&path).expect("save");
        let mut updated = file.clone();
        updated.providers[0].api_key = Some("sk-very-secret".into());
        updated.save_path(&path).expect("save again");

        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&path.with_extension("json.bak")), 0o600);
        assert!(!path.with_extension("json.tmp").exists());
        let backup = ProvidersFile::parse(
            &std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(backup.providers[0].api_key, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_and_load_roundtrip_with_missing_file() {
        let dir = std::env::temp_dir().join(format!(
            "lc-provider-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");

        assert_eq!(ProvidersFile::load_path(&path).unwrap(), None);
        let mut file = ProvidersFile::parse(VALID).expect("parse");
        file.save_path(&path).expect("save");
        assert_eq!(ProvidersFile::load_path(&path).unwrap().unwrap(), file);
        assert!(!path.with_extension("json.bak").exists());

        file.providers[0].api_key = Some("sk-updated".into());
        file.save_path(&path).expect("save again");
        assert_eq!(ProvidersFile::load_path(&path).unwrap().unwrap(), file);
        let bak_path = path.with_extension("json.bak");
        assert!(bak_path.exists());
        let backup = ProvidersFile::parse(&std::fs::read_to_string(&bak_path).unwrap()).unwrap();
        assert_eq!(backup.providers[0].api_key, None);
        assert_eq!(backup.providers[0].label, "OpenAI");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
