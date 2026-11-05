//! config 模块单测。全部使用独立临时目录，不碰真实 `~/.lite-code`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use lc_proto::{NodeSource, SemVer};

use super::*;

fn node_rt() -> NodeRuntime {
    NodeRuntime {
        path: r"C:\tools\node-v22.14.0\node.exe".into(),
        version: SemVer::new(22, 14, 0),
        source: NodeSource::Manual,
        resolved_at: "2026-11-05T08:09:10Z".into(),
    }
}

/// 唯一临时目录（进程 id + 计数器），测试结束尽力清理。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lc-config-test-{}-{}-{tag}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- format_rfc3339 ----

#[test]
fn rfc3339_known_epochs() {
    assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
    // 闰日：2000-02-29
    assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    // 里程碑时刻
    assert_eq!(format_rfc3339(1_234_567_890), "2009-02-13T23:31:30Z");
    // 2026-01-01
    assert_eq!(format_rfc3339(1_767_225_600), "2026-01-01T00:00:00Z");
    // 日内偏移
    assert_eq!(
        format_rfc3339(1_767_225_600 + 45_296),
        "2026-01-01T12:34:56Z"
    );
}

#[test]
fn rfc3339_now_is_well_formed() {
    let s = now_rfc3339();
    assert_eq!(s.len(), 20, "{s}");
    assert!(s.ends_with('Z'), "{s}");
    assert_eq!(s.as_bytes()[4], b'-');
    assert_eq!(s.as_bytes()[10], b'T');
}

// ---- 解析 / 序列化 ----

#[test]
fn round_trip_preserves_extra_fields() {
    let text = r#"{
  "future_setting": { "enabled": true },
  "node_runtime": {
    "path": "C:\\tools\\node.exe",
    "version": { "major": 22, "minor": 14, "patch": 0 },
    "source": "manual",
    "resolved_at": "2026-11-05T08:09:10Z"
  }
}"#;
    let cfg = parse_config(text).expect("parse");
    let rt = cfg.node_runtime.clone().expect("node_runtime");
    assert_eq!(rt.version, SemVer::new(22, 14, 0));
    assert_eq!(rt.source, NodeSource::Manual);
    assert_eq!(
        cfg.extra.get("future_setting"),
        Some(&serde_json::json!({ "enabled": true }))
    );

    // 写回后未知字段仍在
    let out = serialize_config(&cfg);
    let back: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(back.get("future_setting").is_some(), "extra 丢失：{out}");
    assert_eq!(
        back.get("node_runtime").and_then(|n| n.get("source")),
        Some(&serde_json::json!("manual"))
    );
}

#[test]
fn empty_and_absent_node_runtime() {
    let cfg = parse_config("{}").unwrap();
    assert!(cfg.node_runtime.is_none());
    assert_eq!(serialize_config(&cfg), "{}\n");
}

#[test]
fn invalid_json_reports_parse_error() {
    let err = parse_config("{ not json").unwrap_err();
    assert!(matches!(err, ConfigError::Parse(_)), "{err:?}");
}

// ---- JsonConfigStore / write_atomic ----

#[test]
fn store_missing_file_is_default_config() {
    let dir = TempDir::new("missing");
    let store = JsonConfigStore::new(dir.0.join("nested").join("config.json"));
    assert_eq!(store.load().unwrap(), AppConfig::default());
    assert!(store.load().unwrap().node_runtime.is_none());
}

#[test]
fn store_round_trip_and_bak_on_rewrite() {
    let dir = TempDir::new("roundtrip");
    let path = dir.0.join("config.json");
    let store = JsonConfigStore::new(path.clone());

    // 第一次写：无 .bak
    store_node_runtime(&store, node_rt()).unwrap();
    assert!(!with_suffix(&path, "bak").exists(), "首次写不该有备份");
    assert_eq!(
        load_node_runtime(&store).unwrap(),
        Some(node_rt()),
        "内容应一致"
    );

    // 第二次写：.bak 保存旧值
    let mut second = node_rt();
    second.version = SemVer::new(24, 0, 1);
    second.resolved_at = "2026-11-05T09:00:00Z".into();
    store_node_runtime(&store, second).unwrap();

    let bak = std::fs::read_to_string(with_suffix(&path, "bak")).unwrap();
    assert_eq!(parse_config(&bak).unwrap().node_runtime, Some(node_rt()));

    let cur = std::fs::read_to_string(&path).unwrap();
    let cur = parse_config(&cur).unwrap();
    assert_eq!(cur.node_runtime.unwrap().version, SemVer::new(24, 0, 1));

    // 临时文件不残留
    assert!(!with_suffix(&path, "tmp").exists());
}

#[test]
fn write_atomic_creates_parent_dirs() {
    let dir = TempDir::new("deep");
    let path = dir.0.join("a").join("b").join("config.json");
    write_atomic(&path, "{}\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
}
