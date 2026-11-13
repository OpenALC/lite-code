//! `--webview-selftest`：插件 webview iframe 沙箱的运行时自测模式
//! （docs/plans/plugin-webview.md §6 M3 双平台启用前提的 Linux 侧验证）。
//!
//! 仅 CI（或开发者显式传参）使用：`xvfb-run ./target/debug/lc-desktop
//! --webview-selftest` 拉起应用后，Rust 侧向主 webview 反复注入一段幂等
//! 自测脚本（[INJECTED_JS]，复用 UI 的 DOM 操作：活动栏「插件」→ 插件卡片
//! webview「打开视图」→ iframe 出现），脚本在页面上下文轮询
//! `ext_webview_state_get` 直至 demo 插件经握手 + MessageChannel 桥写入的
//! greet 值可读回；结果经 [webview_selftest_report] 命令回传，守护线程打印
//! `WEBVIEW_SELFTEST: PASS/FAIL <detail>` 标记行并以 0/1 退出（60s 硬超时
//! 强退 1）。
//!
//! 与产品路径的隔离：本模块只在命令行含 [SELFTEST_ARG] 时启动；注入脚本不
//! 改动任何产品行为（只点既有按钮、读既有状态）；正常模式下报告命令无人
//! 调用、守护线程不启动，命令注册本身不引入任何产品面变化。

use std::io::Write as _;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, State};

/// 触发自测模式的命令行参数。
pub(crate) const SELFTEST_ARG: &str = "--webview-selftest";

/// 自测硬超时（总预算，含页面加载与桥往返）。
const SELFTEST_DEADLINE: Duration = Duration::from_secs(60);
/// 注入脚本的轮询间隔：页面导航完成前 eval 会丢，幂等重复注入兜底。
const EVAL_INTERVAL: Duration = Duration::from_millis(700);

/// 判定通过的状态值（demo 插件 setState 写入，docs/plans/plugin-webview.md §3.8）。
const GREET_VALUE: &str = "hello-from-sandbox";

/// 注入主 webview 的自测脚本：幂等（全局守卫），异步推进各阶段并以
/// `webview_selftest_report` 回传结果。各阶段预算合计 < 60s 硬超时。
/// `__GREET_VALUE__` 占位符在 [injected_js] 里替换为 [`GREET_VALUE`]，
/// 判定值只在本模块维护一份。
const INJECTED_JS: &str = r#"
(() => {
  if (window.__lcWebviewSelftestStarted) return;
  window.__lcWebviewSelftestStarted = true;
  const GREET = '__GREET_VALUE__';
  const PLUGIN_ID = 'demo.webview-sandbox';
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const report = (pass, detail) => {
    window.__TAURI_INTERNALS__.invoke('webview_selftest_report', { pass, detail }).catch(() => {});
  };
  const until = async (probe, timeoutMs, label) => {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const value = probe();
      if (value) return value;
      if (Date.now() >= deadline) throw new Error('stage-timeout: ' + label);
      await sleep(250);
    }
  };
  (async () => {
    try {
      // 1) 页面与应用就绪（CI 冷启动下 Vite 产物加载需要几秒）。
      await until(
        () => document.readyState === 'complete' && window.__TAURI_INTERNALS__,
        15000,
        'document-ready',
      );
      // 2) 活动栏「插件」按钮（zh-CN 默认标题；en 兜底）打开插件管理标签。
      const activity = document.querySelector('.activity .act-btn[title="插件"]')
        || document.querySelector('.activity .act-btn[title="Extensions"]');
      if (!activity) throw new Error('extensions-activity-button-not-found');
      activity.click();
      // 3) 等插件列表渲染出 webview 声明条目（.command-title == webview id）。
      const item = await until(() => {
        const found = [...document.querySelectorAll('.command-item')].find((el) => {
          const title = el.querySelector('.command-title');
          return title && title.textContent.trim() === 'panel';
        });
        return found || null;
      }, 15000, 'webview-command-item');
      const button = item.querySelector('.run-button');
      if (!button || button.disabled) throw new Error('webview-run-button-unavailable');
      button.click();
      // 4) iframe 挂载（src 由宿主生成器产出，lc-plugin://…#nonce）。
      await until(
        () => document.querySelector('iframe.wv-frame'),
        15000,
        'iframe-mount',
      );
      // 5) demo 子页握手 + 桥 setState 后，父侧读回应为 GREET——桥往返断言。
      const deadline = Date.now() + 15000;
      let last = 'value-not-written';
      for (;;) {
        try {
          const res = await window.__TAURI_INTERNALS__.invoke('ext_webview_state_get', {
            pluginId: PLUGIN_ID,
            key: 'greet',
          });
          if (res && res.value === GREET) {
            report(true, 'greet=' + GREET + ' (lc-plugin asset + handshake + MessageChannel setState/getState)');
            return;
          }
          last = 'value-mismatch: ' + JSON.stringify(res && res.value);
        } catch (error) {
          last = String(error);
        }
        if (Date.now() >= deadline) break;
        await sleep(500);
      }
      report(false, 'greet-poll-timeout: ' + last);
    } catch (error) {
      report(false, String((error && error.message) || error));
    }
  })();
})();
"#;

/// 自测结果（注入脚本经 [webview_selftest_report] 回传）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelftestReport {
    pub(crate) pass: bool,
    pub(crate) detail: String,
}

/// 自测传输通道：守护线程持有接收端；命令处理把发送端装进来。
/// 正常模式恒为 None——命令注册但无人调用，零产品影响。
#[derive(Default)]
pub(crate) struct SelftestState {
    tx: Mutex<Option<Sender<SelftestReport>>>,
}

impl SelftestState {
    fn install(&self, tx: Sender<SelftestReport>) {
        if let Ok(mut guard) = self.tx.lock() {
            *guard = Some(tx);
        }
    }

    fn sender(&self) -> Option<Sender<SelftestReport>> {
        let guard = self.tx.lock().ok()?;
        guard.clone()
    }
}

/// 注入脚本的结果回传命令。仅 `--webview-selftest` 的注入脚本会调用；
/// 重复调用只取第一次（在途即终态）。
#[tauri::command]
pub(crate) fn webview_selftest_report(state: State<'_, SelftestState>, pass: bool, detail: String) {
    if let Some(tx) = state.sender() {
        let _ = tx.send(SelftestReport { pass, detail });
    }
}

/// 启动自测：独立临时工作区（不向运行目录写任何文件）、注入循环、守护线程。
pub(crate) fn start(app: &AppHandle) {
    let workspace =
        std::env::temp_dir().join(format!("lc-webview-selftest-ws-{}", std::process::id()));
    if let Err(error) = std::fs::create_dir_all(&workspace) {
        fail_and_exit(&format!("workspace-temp-dir: {error}"));
    }
    app.state::<crate::WorkspaceState>().set(workspace);

    let (tx, rx) = channel::<SelftestReport>();
    app.state::<SelftestState>().install(tx);

    // 注入循环：页面导航完成前的 eval 会被丢弃，幂等脚本 + 周期重试兜底。
    let injector = app.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + SELFTEST_DEADLINE;
        while Instant::now() < deadline {
            if let Some(window) = injector.get_webview_window("main") {
                let _ = window.eval(injected_js());
            }
            std::thread::sleep(EVAL_INTERVAL);
        }
    });

    // 守护线程：等报告 → 打印标记行 → 按结果退出；60s 无报告按超时失败。
    std::thread::spawn(move || match rx.recv_timeout(SELFTEST_DEADLINE) {
        Ok(report) => {
            let code = if report.pass { 0 } else { 1 };
            println!(
                "WEBVIEW_SELFTEST: {} {}",
                if report.pass { "PASS" } else { "FAIL" },
                report.detail
            );
            let _ = std::io::stdout().flush();
            std::process::exit(code);
        }
        Err(_) => fail_and_exit("timeout(60s): no report from injected selftest script"),
    });
}

fn fail_and_exit(detail: &str) -> ! {
    eprintln!("WEBVIEW_SELFTEST: FAIL {detail}");
    let _ = std::io::stdout().flush();
    std::process::exit(1);
}

/// 注入脚本本体：占位符替换为唯一真源 [`GREET_VALUE`]。
fn injected_js() -> String {
    let script = INJECTED_JS.replace("__GREET_VALUE__", GREET_VALUE);
    debug_assert!(script.contains(GREET_VALUE));
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_js_is_self_contained_and_idempotent() {
        let script = injected_js();
        assert!(script.contains("window.__lcWebviewSelftestStarted"));
        // 幂等守卫必须先于一切副作用。
        let guard_at = script.find("__lcWebviewSelftestStarted = true").unwrap();
        let click_at = script.find("activity.click()").unwrap();
        assert!(guard_at < click_at);
        // 断言目标值与回传命令名钉死在本模块内。
        assert!(script.contains(GREET_VALUE));
        assert!(!script.contains("__GREET_VALUE__"));
        assert!(script.contains("webview_selftest_report"));
        assert!(script.contains("ext_webview_state_get"));
        // 注入脚本不得触碰产品存储之外的写路径。
        assert!(!script.contains("localStorage"));
    }

    #[test]
    fn report_command_forwards_first_result() {
        let state = SelftestState::default();
        assert!(state.sender().is_none());
        let (tx, rx) = channel();
        state.install(tx);
        let tx = state.sender().expect("sender installed");
        tx.send(SelftestReport {
            pass: true,
            detail: "ok".into(),
        })
        .expect("send");
        let report = rx.recv().expect("recv");
        assert!(report.pass);
        assert_eq!(report.detail, "ok");
    }
}
