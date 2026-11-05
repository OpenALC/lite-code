//! Tauri 薄壳（docs/ARCHITECTURE.md L2）。
//!
//! 只做三件事：命令注册转发、窗口/托盘生命周期（§6）、单实例互斥。
//! 业务逻辑全部下沉 lc-core：fs 命令的路径校验（`ensure_within_root`）与
//! 按层列出（`read_dir_level`）都来自 `lc_core::fs`，返回值直接复用
//! `lc_proto::CommandResult`，错误经 `FsError::code()` 映射为 `ErrorBody`。
//!
//! 生命周期（docs/ARCHITECTURE.md §6 起步版）：
//! 二次启动 → 唤回主窗口；关闭请求 → 隐藏窗口（核心常驻）；托盘"退出"才真正退出。

use std::path::{Path, PathBuf};

use lc_core::fs::{
    DirLevel, FsError, RealPathResolver, ensure_within_root, read_dir_level, write_file_text,
};
use lc_proto::{CommandResult, ContractMeta, ErrorBody, ErrorCode, NodeStatus, ShellInfo};

// get_webview_window 来自 Manager trait（仅桌面侧的托盘/单实例路径用到）
#[cfg(desktop)]
use tauri::Manager;

/// workspace 根由传输层注入（docs/PLAN.md R3）：`LC_WORKSPACE_ROOT` 供
/// dev/测试显式覆盖，否则用进程当前目录。fs 命令不接受前端传入的 root。
fn current_workspace_root() -> Option<PathBuf> {
    std::env::var_os("LC_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
}

fn workspace_unavailable() -> ErrorBody {
    ErrorBody::new(ErrorCode::NotFound, "workspace root unavailable")
}

/// 当前工作区根（文件树与 fs 命令的安全边界）。前端据此拿到文件树起点，
/// 不再依赖 URL query 或手填路径。
#[tauri::command]
fn workspace_root() -> CommandResult {
    CommandResult::WorkspaceRoot {
        root: current_workspace_root().map(|p| lc_core::fs::display_path(&p)),
    }
}

/// 转发 lc-core 的 Node 探测。惰性触发由前端控制，这里不做缓存。
#[tauri::command]
fn node_detect() -> NodeStatus {
    lc_core::runtime::probe_real()
}

/// 转发 shell 探测（终端与 Agent 执行依赖，docs/RUNTIME.md §8）。
#[tauri::command]
fn shell_detect() -> Option<ShellInfo> {
    lc_core::runtime::detect_shell_real()
}

/// 契约元信息（"关于"页与 lc-cli 共用同一来源）。
#[tauri::command]
fn contract_info() -> ContractMeta {
    lc_proto::ContractMeta::current()
}

/// 目录按层返回（docs/ARCHITECTURE.md §5.2：不整树序列化）。
/// 契约的 `CoreCommand::FsReadDir { path }` 只带 path，root 由传输层注入；
/// 前端传入的路径必须落在 workspace 根内（docs/PLAN.md R3）。
#[tauri::command]
fn fs_read_dir(path: String) -> Result<CommandResult, ErrorBody> {
    let root = current_workspace_root().ok_or_else(workspace_unavailable)?;
    let level = list_dir(&root, &path)?;
    Ok(CommandResult::DirListing {
        path: level.path,
        entries: level.entries,
        // generation 由 watch lane 的事件流提供（fs:tree），命令快照阶段恒 0；
        // 前端跳变检测在监听接入前不生效，见 docs/ARCHITECTURE.md §5.2。
        generation: 0,
    })
}

/// 读文件为文本（契约 v0 语义：`CommandResult::FileContent { path, text }`；
/// 原始字节传输在后续里程碑改 `tauri::ipc::Response`，docs/ARCHITECTURE.md §5.2）。
#[tauri::command]
fn fs_read_file(path: String) -> Result<CommandResult, ErrorBody> {
    let root = current_workspace_root().ok_or_else(workspace_unavailable)?;
    let (display, text) = read_file_text(&root, &path)?;
    Ok(CommandResult::FileContent {
        path: display,
        text,
    })
}

/// 保存文本文件（P1 Editor UI lane）。目标必须落在 workspace 根内，允许新建。
#[tauri::command]
fn fs_write_file(path: String, content: String) -> Result<CommandResult, ErrorBody> {
    let root = current_workspace_root().ok_or_else(workspace_unavailable)?;
    write_text(&root, &path, &content)?;
    Ok(CommandResult::Ack)
}

// ==== 纯逻辑层（无 Tauri 运行时依赖，单测直接覆盖） ====

/// 列一层目录：lc-core 校验 + 列举，失败映射为契约 `ErrorBody`。
fn list_dir(root: &Path, path: &str) -> Result<DirLevel, ErrorBody> {
    read_dir_level(root, Path::new(path), &RealPathResolver).map_err(|e| fs_error_body(&e))
}

/// 根内校验后读文件，返回（展示路径, 文本）。非 UTF-8 归入 `Io` 码
/// （契约 v0 只承诺文本；二进制走原始字节的方案见上）。
fn read_file_text(root: &Path, path: &str) -> Result<(String, String), ErrorBody> {
    let canonical = ensure_within_root(root, Path::new(path), &RealPathResolver)
        .map_err(|e| fs_error_body(&e))?;
    let text =
        std::fs::read_to_string(&canonical).map_err(|e| fs_error_body(&io_to_fs(&canonical, e)))?;
    Ok((lc_core::fs::display_path(&canonical), text))
}

/// 根内校验后写入文本，返回展示路径。
fn write_text(root: &Path, path: &str, content: &str) -> Result<String, ErrorBody> {
    write_file_text(root, Path::new(path), content, &RealPathResolver)
        .map_err(|e| fs_error_body(&e))
}

/// 错误映射：`FsError::code()`（契约错误码）→ `ErrorBody`。
/// message 用 FsError 的 Display（含展示路径），lane 私有细节不进契约。
fn fs_error_body(e: &FsError) -> ErrorBody {
    ErrorBody::new(e.code(), e.to_string())
}

/// `std::io::Error` → `FsError`：`NotFound` 单列（契约有对应码），其余归 `Io`。
/// lc-core 的同名辅助是 `pub(crate)`，薄壳侧按同一规则实现（两处语义一致）。
fn io_to_fs(path: &Path, e: std::io::Error) -> FsError {
    let path = lc_core::fs::display_path(path);
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound { path }
    } else {
        FsError::Io {
            path,
            message: e.to_string(),
        }
    }
}

// ==== 生命周期（docs/ARCHITECTURE.md §6） ====

/// 托盘菜单：显示主窗口 / 退出。标签与 docs/ARCHITECTURE.md §6 的常驻语义对应；
/// 运行时才创建，Linux CI 无显示环境只编译不执行。
#[cfg(desktop)]
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("lite-code-tray")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Lite Code")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            // 唯一真正退出的入口：窗口关闭只是隐藏（§6 常驻语义）
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 唤回主窗口：显示 + 取消最小化 + 聚焦（单实例二次启动与托盘共用）。
#[cfg(desktop)]
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(desktop)]
    let builder = {
        builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 二次启动：唤回已有实例的主窗口（§6：单实例检查）
            show_main_window(app);
        }))
    };

    builder
        .invoke_handler(tauri::generate_handler![
            node_detect,
            shell_detect,
            contract_info,
            workspace_root,
            fs_read_dir,
            fs_read_file,
            fs_write_file
        ])
        .setup(|app| {
            #[cfg(desktop)]
            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭请求 → 隐藏窗口、核心常驻（§6 起步版：窗口销毁语义 +
            // 托盘常驻；"退出"只经托盘菜单）。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running lite-code");
}

// ==== 单测：纯逻辑层不碰 Tauri 运行时，临时目录与 lc-core fs/tests.rs 同策略 ====

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use lc_proto::{CommandResult, ErrorCode};

    use super::{list_dir, read_file_text, write_text};

    /// 进程内唯一临时目录（lc-core 不引 tempfile，薄壳同样不引入新依赖）。
    fn temp_root(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lc-desktop-{}-{}-{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn code_of(result: &Result<impl Sized, lc_proto::ErrorBody>) -> ErrorCode {
        result.as_ref().err().expect("expected error").code
    }

    #[test]
    fn list_dir_returns_sorted_single_level() {
        let root = temp_root("list");
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        // 嵌套文件不应出现：按层返回，不递归
        fs::create_dir_all(root.join("sub/inner")).unwrap();
        fs::write(root.join("sub/inner/deep.txt"), "d").unwrap();

        let level = list_dir(&root, root.to_str().unwrap()).expect("list ok");
        let names: Vec<&str> = level.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt", "sub"]);
        assert_eq!(level.entries[2].kind, lc_proto::EntryKind::Dir);
    }

    #[test]
    fn list_dir_maps_outside_root_to_contract_code() {
        let root = temp_root("outside");
        let outside = std::env::temp_dir();
        let err = list_dir(&root, outside.to_str().unwrap());
        assert_eq!(code_of(&err), ErrorCode::PathOutsideRoot);
    }

    #[test]
    fn list_dir_maps_missing_to_not_found() {
        let root = temp_root("missing");
        let err = list_dir(&root, root.join("nope").to_str().unwrap());
        assert_eq!(code_of(&err), ErrorCode::NotFound);
    }

    #[cfg(windows)]
    #[test]
    fn list_dir_dot_dot_escape_is_rejected() {
        // `root\..` 在文本层逃逸：canonicalize 折叠成父目录后必须落回 root 内校验
        // （Win32 对组件尾部点号的剥离规则会让 `..\..` 形态不确定，这里用单层 `..`）
        let root = temp_root("escape");
        let escaped = format!(r"{}\..", root.display());
        let err = list_dir(&root, &escaped);
        assert_eq!(code_of(&err), ErrorCode::PathOutsideRoot);
    }

    #[test]
    fn read_file_returns_text_and_display_path() {
        let root = temp_root("read");
        fs::write(root.join("hello.rs"), "fn main() {}\n").unwrap();
        let (path, text) =
            read_file_text(&root, root.join("hello.rs").to_str().unwrap()).expect("read ok");
        assert_eq!(text, "fn main() {}\n");
        // Windows verbatim 前缀（\\?\）已剥掉，前端拿到的是展示路径
        assert!(!path.contains(r"\\?\"));
        assert!(path.ends_with("hello.rs"));
    }

    #[test]
    fn read_file_maps_outside_root_and_missing() {
        let root = temp_root("read-guard");
        let outside = read_file_text(&root, std::env::temp_dir().to_str().unwrap());
        assert_eq!(code_of(&outside), ErrorCode::PathOutsideRoot);

        let missing = read_file_text(&root, root.join("no.rs").to_str().unwrap());
        assert_eq!(code_of(&missing), ErrorCode::NotFound);
    }

    #[test]
    fn read_file_non_utf8_maps_to_io() {
        let root = temp_root("binary");
        fs::write(root.join("bin.dat"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
        let err = read_file_text(&root, root.join("bin.dat").to_str().unwrap());
        assert_eq!(code_of(&err), ErrorCode::Io);
    }

    #[test]
    fn write_file_roundtrip_and_guard() {
        let root = temp_root("write");
        let target = root.join("saved.txt");
        let path = write_text(&root, target.to_str().unwrap(), "saved").expect("write ok");
        assert!(path.ends_with("saved.txt"));

        let (_, text) = read_file_text(&root, target.to_str().unwrap()).expect("read back");
        assert_eq!(text, "saved");

        let outside = std::env::temp_dir().join("lc-desktop-write-outside.txt");
        let _ = fs::remove_file(&outside);
        let err = write_text(&root, outside.to_str().unwrap(), "x");
        assert_eq!(code_of(&err), ErrorCode::PathOutsideRoot);
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn command_results_carry_contract_kind() {
        // 序列化形状回归：kind tag 与 bindings/lc-proto.d.ts 的 CommandResult 对齐
        let listing = CommandResult::DirListing {
            path: "D:/p".into(),
            entries: vec![],
            generation: 0,
        };
        assert_eq!(
            serde_json::to_value(&listing).unwrap()["kind"],
            serde_json::json!("dir_listing")
        );
        let content = CommandResult::FileContent {
            path: "D:/p/a.rs".into(),
            text: "x".into(),
        };
        assert_eq!(
            serde_json::to_value(&content).unwrap()["kind"],
            serde_json::json!("file_content")
        );
    }
}
