# 架构设计

## 1. 分层

```
┌───────────────────────────────────────────────────────────┐
│ L4  UI          WebView2 / WebKitGTK / ArkWeb             │
│                 Vue 3 + Monaco + xterm.js + dockview       │
├───────────────────────────────────────────────────────────┤
│ L3  契约层      crates/lc-proto                           │
│                 DTO · Command · Event · ErrorCode          │
│                 → 生成 TS 绑定（为 ArkTS 预留同源）         │
├───────────────────────────────────────────────────────────┤
│ L2  Tauri 薄壳  apps/desktop/src-tauri                    │
│                 命令注册 · 窗口 · 托盘 · 单实例 · 更新器    │
├───────────────────────────────────────────────────────────┤
│ L1  核心服务    crates/lc-core   （零 tauri 依赖）          │
│                 runtime · fs · watch · process · git       │
│                 ssh · lsp · agent · mcp · llm · config     │
├───────────────────────────────────────────────────────────┤
│ L0  外部进程    sidecars/（可选，系统 Node 运行）           │
│                 MCP servers · db-bridge                    │
└───────────────────────────────────────────────────────────┘
```

`lc-cli` 是 `lc-core` 的第二个前端，用于 headless 验证与 CI 回归。**它不是附属品，是并行开发的验证杠杆。**

## 2. 仓库结构

```
lite-code/
├─ rust-toolchain.toml          # channel = "1.99.1"
├─ Cargo.toml                   # workspace
├─ crates/
│  ├─ lc-proto/                 # DTO + codegen
│  ├─ lc-core/                  # 服务实现
│  │  └─ src/{runtime,fs,watch,process,git,ssh,lsp,agent,mcp,llm,config,secrets}/
│  └─ lc-cli/                   # headless 前端
├─ apps/
│  ├─ desktop/src-tauri/        # 薄壳
│  └─ web/                      # Vue 3
│     └─ src/{editor,tabs,tree,agent,settings,services}/
├─ sidecars/{db-bridge,mcp-runner}/
└─ docs/
```

## 3. 关键抽象

这些 trait 是跨平台适配的全部成本所在。做到位，鸿蒙只是"再写一个外壳"；做不到，就是"重写"。

| Trait | 职责 | Windows | Linux | HarmonyOS |
|---|---|---|---|---|
| `EventSink` | 服务 → UI 单向下行 | Tauri `Channel` | 同左 | NAPI callback |
| `RuntimeProvider` | 提供 Node 能力 | 系统探测 | 系统探测 | 系统探测 + 可选 libnode |
| `ProcessSupervisor` | 子进程生命周期 | Job Object | 进程组 + PDEATHSIG | NAPI 持有 |
| `SecretStore` | 密钥 | Credential Manager | libsecret | Preferences + 加密 |
| `PathProvider` | 配置/缓存/数据目录 | `dirs` | `dirs` (XDG) | 沙箱目录 |
| `GitBackend` | Git 操作 | 系统 `git.exe` | 系统 `git` | 需移植或禁用 |

## 4. 核心不做 Tauri 依赖的写法

```rust
// lc-core 只认识这个
pub trait CoreHandle: Send + Sync {
    fn emit(&self, event: CoreEvent);
    fn paths(&self) -> &dyn PathProvider;
    fn secrets(&self) -> &dyn SecretStore;
}

// src-tauri 里实现它
impl CoreHandle for TauriHandle {
    fn emit(&self, event: CoreEvent) {
        let _ = self.app.emit("core://event", event);
    }
    // ...
}
```

将来要拆成独立守护进程，只需再加一个 socket transport 实现，`lc-core` 一行不改。

## 5. 模块选型

### 5.1 编辑器

Monaco，单方案。大文件分级降级：

| 文件大小 | 策略 |
|---|---|
| < 5 MB | 完整能力 |
| 5–20 MB | 关闭 occurrences、折叠计算、minimap，保留着色 |
| > 20 MB | 只读预览 + "用系统编辑器打开" |

Worker 用 Vite 的 `?worker` 引入，禁止 CDN loader（离线不可用）。Monaco 走动态 `import()`，首次打开文件时才加载。

### 5.2 文件系统

- 遍历：`ignore` crate（天然支持 `.gitignore` / `.ignore`）
- 监听：`notify` + `notify-debouncer-full`（去抖 200ms）
- Linux inotify watch 上限溢出时降级为轮询
- 目录**按层返回**，不整树序列化
- 事件携带 `generation`，前端发现跳变即重拉
- 读文件用 `tauri::ipc::Response` 返回原始字节，避免 base64 往返

### 5.3 Git

系统 `git` CLI 为主，`gix` 只在遇到超大仓库性能问题时作为备选实现。

```rust
let out = Command::new(&git_path)
    .args(["--no-pager", "-c", "color.ui=false", "status",
           "--porcelain=v2", "-z", "--branch"])
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("LC_ALL", "C")
    .output()?;
```

要点：参数走数组、`-z` 分隔、永不加 `--color`、认证走系统 credential helper。

### 5.4 SSH / 终端

| 用途 | 方案 |
|---|---|
| 交互终端 | 系统 `ssh` + `portable-pty` + xterm.js |
| 程序化传输 | `russh`（纯 Rust） |

`portable-pty` 同时是 L2 Agent 会话宿主的基础设施——一个 PTY 管理器服务两个用途。

#### 5.4.1 Shell 选择

**单一 shell 原则**：任何一个执行上下文，只暴露一种 shell。不提供"自动猜"，不让模型在 PowerShell / cmd / bash 之间做选择——混用方言是命令失败的头号原因。

| 平台 | 默认 shell | 检测方式 | 缺失时 |
|---|---|---|---|
| Windows | **PowerShell 7**（`pwsh`） | 绝对路径 + `where pwsh`，再跑版本校验要求 major ≥ 7 | 提示用户用 winget 安装 |
| Linux | **bash** | `/bin/bash`、`/usr/bin/bash`，再校验 `bash --version` | 极少缺失；缺失则报错并列出可用 shell |
| HarmonyOS | 暂不支持交互终端 | — | 功能隐藏 |

Windows 探测顺序（复用 RUNTIME.md 的绝对路径优先策略，不依赖继承的 PATH）：

```
%ProgramFiles%\PowerShell\7\pwsh.exe
%ProgramFiles%\PowerShell\7-preview\pwsh.exe
where pwsh  → 取第一条结果
校验：pwsh -NoLogo -NoProfile -Command "$PSVersionTable.PSVersion.Major"
      结果 ≥ 7 才算命中；< 7 或执行失败则视为未安装
```

**明确不使用的 shell**：

- `cmd.exe` —— 不作为默认，也不作为自动降级项
- Windows PowerShell 5.1 —— 仅在用户显式选择时使用（见下）
- Git Bash / MSYS2 —— 开发机上很常见，但会引入第三种方言，不使用

#### 5.4.2 未安装时的引导

```
┌─ 未检测到 PowerShell 7 ────────────────────────────┐
│                                                     │
│  Lite Code 的终端与 Agent 命令执行依赖 PowerShell 7。│
│                                                     │
│  winget install --id Microsoft.PowerShell -e        │
│                            --source winget          │
│                                                     │
│  [ 复制命令 ]  [ 直接安装 ]  [ 本次使用 5.1 ]       │
│  [ 打开下载页 ]                                     │
└─────────────────────────────────────────────────────┘
```

- **`[直接安装]`** 在一个可见终端里跑 winget，让用户看到进度和可能出现的 UAC 提示；不静默执行，不隐藏输出。
- 安装完成后必须**重新探测**并校验版本，而不是假定成功。
- winget 本身缺失（老版本 Win10）时，隐藏 `[直接安装]`，只留复制命令与下载页。
- `[本次使用 5.1]` 是显式降级：只对本次会话生效，UI 上标注"兼容模式"，并让环境契约同步声明 `windows-powershell-5.1`，模型据此避免 `&&`、`??`、三元运算符这类 PS7 独有语法。

#### 5.4.3 Profile 策略（两种用途分开）

| 用途 | 启动方式 | 理由 |
|---|---|---|
| 交互终端 | `pwsh -NoLogo`（加载 profile） | 用户要 alias、venv、conda 初始化生效 |
| Agent 工具执行 | `pwsh -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass` | 确定性：profile 里的别名和函数会改变命令语义，破坏可复现性 |

Linux 同理：交互用 `bash`（加载 `.bashrc`），工具执行用 `bash --noprofile --norc -c`。

#### 5.4.4 输出与退出码

PowerShell 的两个老问题是输出编码和退出码不可靠，必须在包装层解决：

- **编码**：包装脚本开头设 `[Console]::OutputEncoding = [Text.UTF8Encoding]::new()`，Rust 侧按 UTF-8 解码 stdout/stderr。不要依赖系统代码页。
- **退出码**：`$LASTEXITCODE` 只在调用了外部原生命令后有效，纯 PowerShell 错误默认不设非零退出码。包装脚本显式收敛：

```powershell
try {
    <user command>
    exit $(if ($LASTEXITCODE -ne $null) { $LASTEXITCODE } else { 0 })
} catch {
    $_ | Out-String | Write-Error
    exit 1
}
```

- **stdout/stderr 分离**：PowerShell 的错误流会混进 stdout，包装层用 `2>&1` 重定向后按流前缀区分，或在 Rust 侧保留原始流边界。

### 5.5 数据库

可选 sidecar，纯 JS 驱动（`pg` / `mysql2` / `mssql`），SQLite 优先 `node:sqlite`，低于 Node 22.5 降级 `sql.js`。esbuild 预打包为单文件，运行时不需要 `node_modules`。

### 5.6 LSP

通用 JSON-RPC over stdio 客户端。**不捆绑 language server**，只检测用户已安装的 `rust-analyzer` / `gopls` / `pyright`。v1 只保证 TS/JS（用 Monaco 内置 worker）。

### 5.7 HarmonyOS / ArkWeb（P6 spike 结论）

2026-11-05 在本机 DevEco 26.0.0（API 26）+ Relay_Phone_API26 模拟器完成 spike，
工程在 `spikes/harmony-arkweb`：

| 门禁项 | 结论 | 说明 |
|---|---|---|
| ArkWeb 跑现有前端 | ✅ | 通过 HTTP dev server 加载 `apps/web`，布局/文件树/编辑器/任务栏均渲染 |
| rawfile 直载 Vite 产物 | ❌ | `resource://rawfile` 下 ES module 与 CSS 被 CORS 拒绝（origin null）；需 HTTP 或单文件内联 |
| NAPI 承载 CoreHandle | ✅ | `libentry.so` 导出 `createHandle()`，ArkTS 收到 `{id,native}` 并显示 |
| Monaco 渲染 | ✅ | 语法高亮、折叠、缩略图可用 |
| Monaco 内存 | ⚠️ | 模拟器 PSS：应用 ~142MB + render ~275MB（完整前端 + Monaco） |
| Monaco 输入法 | ⚠️ | `uitest uiInput inputText` 未验证出文本；需真机 IME 复核 |

**决策**：按 MILESTONES 门禁“只过前两项 → 只读 + 对话”，鸿蒙首版不启用可编辑 Monaco；
保留 ArkWeb + NAPI 的只读代码预览与 Agent 会话。若要升级为可编辑，需要先解决
rawfile 资源加载（HTTP/自定义 scheme/单文件构建）、内存占用与 IME 三项。

## 6. 生命周期

```
启动 → 单实例检查 → 加载配置 → 惰性探测 Node（不阻塞 UI）
     → 创建窗口 → 恢复工作区状态

最小化/关闭 → 销毁 WebView（释放 100MB+）
            → 核心继续运行（托盘常驻）
            → PTY / Agent 会话 / 子进程不受影响

唤回 → 重建 WebView → 从 Rust 侧拉取状态 → 恢复视图
```

状态分类（这条决定了能否优雅重建）：

| 类型 | 例子 | 存放 |
|---|---|---|
| 可丢弃 | 滚动位置、折叠状态、光标 | 磁盘，重建时恢复 |
| 不可丢弃 | 未保存缓冲、运行中任务、终端会话 | Rust 侧，窗口重建后重新订阅 |

## 7. 构建产物

| 平台 | 产物 |
|---|---|
| Windows x64 | `.msi` + `.exe`(NSIS) + portable |
| Linux x64 / arm64 | `.deb` + `.rpm` + AppImage（依赖系统 webkit2gtk-4.1） |
| HarmonyOS | `.hap`（后续独立工作流） |

不生产内嵌 WebKit 的 AppImage——与"轻量"定位直接矛盾。
