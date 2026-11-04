# Node 运行时子系统

Node 是**可选集成**，不是应用的必要条件。harness、编辑器、文件树、Git、SSH 终端均不依赖它。这让"未找到 Node"从致命错误降级为一次功能提示。

## 1. 状态机

```
                    ┌──────────────┐
     启动 ──────────▶│  Unresolved  │
                    └──────┬───────┘
                           │ 首次需要 Node 时惰性触发
                           ▼
                    ┌──────────────┐
                    │   Probing    │  串行，首个可用即返回
                    └──────┬───────┘
              ┌────────────┴────────────┐
              ▼                         ▼
      ┌──────────────┐          ┌──────────────┐
      │    Ready     │          │  NeedManual  │
      │ + 版本标签    │          └──────┬───────┘
      └──────────────┘                 │
                              ┌────────┼────────┐
                              ▼        ▼        ▼
                          选路径   下载便携版   跳过
                              │        │        │
                              └────┬───┘        │
                                   ▼            ▼
                              Ready        Degraded
```

`Degraded` 是合法终态：应用正常运行，Node 相关入口显示为禁用并附引导。

## 2. 版本策略

**基准 Node 22，低版本不阻断。**

| 版本 | 标签 | 行为 |
|---|---|---|
| ≥ 22 | `Supported` | 全功能 |
| 18 – 21 | `Unverified` | 全功能，状态栏黄色；首次使用 Node 功能时提示一次 |
| < 18 | `Legacy` | 全功能，文案明确"可能失败"；不阻断 |
| 能启动但自检失败 | `Broken` | 跳过该候选，继续下一个 |

设计意图：探测**只回答"能不能跑"**，版本只影响标签。避免用户用 nvm 切版本就弹窗的糟糕体验。

## 3. 候选枚举

按优先级分四档，**同档内按版本号降序**。绝不通过 PATH 环境变量查找 node 本身。

### Windows

```rust
enum Source { Tier1, Tier2, Tier3, RegistryPath }

// Tier 1 —— 官方安装器
%ProgramFiles%\nodejs\node.exe
%ProgramFiles(x86)%\nodejs\node.exe
%LOCALAPPDATA%\Programs\nodejs\node.exe

// Tier 2 —— 版本管理器（glob 匹配，取最高版本）
%NVM_HOME%\v*\node.exe
%APPDATA%\nvm\v*\node.exe
%NVM_SYMLINK%\node.exe
%FNM_DIR%\node-versions\v*\installation\node.exe
%APPDATA%\fnm\node-versions\v*\installation\node.exe
%LOCALAPPDATA%\Volta\tools\image\node\*\node.exe

// Tier 3 —— 包管理器
%ChocolateyInstall%\bin\node.exe
%SCOOP%\apps\nodejs*\current\node.exe
%USERPROFILE%\scoop\apps\nodejs\current\node.exe

// Tier 4 —— 注册表 PATH
HKCU\Environment  → PATH 拆分
HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment → PATH 拆分
```

**Tier 4 读注册表而不是进程 `PATH`**：用户改环境变量后，已运行进程继承的是旧值，这是"我明明装好了却找不到"的头号原因。

### 两个必须处理的陷阱

**Store 别名桩**。`%LOCALAPPDATA%\Microsoft\WindowsApps\node.exe` 是 0 字节 reparse point，执行报"找不到应用"。路径含 `WindowsApps` 时直接标记 `StoreAlias`，不执行。

**shim 转发器**。Chocolatey / Scoop 的 `node.exe` 是转发器，`--version` 正常，但长驻子进程会拖住父进程。首次执行后记录它解析到的真实路径（`std::fs::canonicalize` 对 reparse point 有效），后续走真实路径。

### Linux

```
1. 裸命令 node（此时才用 PATH，因为是用户显式配置的 shell）
2. $NVM_DIR/versions/node/v*/bin/node
3. $HOME/nvm/versions/node/v*/bin/node
4. ~/.nvm/versions/node/v*/bin/node
5. $N_PREFIX/n*/bin/node
6. /usr/local/n/versions/node/*/bin/node
7. /usr/bin/node /usr/local/bin/node /snap/bin/node
```

Desktop 环境通过 `.desktop` 启动时 PATH 可能不含 nvm 的 shell 初始化结果，因此**必须扫描版本目录**，不能只靠裸命令。

## 4. 校验流程

```
for candidate in ordered_candidates():
    if not exists(path)          → record(Missing)
    if is_dir(path)              → record(NotAFile)
    if contains("WindowsApps")   → record(StoreAlias)

    out = spawn(path, ["--version"], timeout=3s, CREATE_NO_WINDOW)
    v   = parse_first(/^v(\d+)\.(\d+)\.(\d+)/, out.stdout + out.stderr)

    if spawn_failed / timeout    → record(SpawnFailed)
    if v.is_none()               → record(Unparsable)
    if v.major < 18              → 仍可作为候选，标记 Legacy

    // 二次自检：排除"exe 在但 DLL 缺失"
    ok = spawn(path, ["-e", "require('fs');process.exit(0)"], timeout=3s)
    if not ok                    → record(GhostInstall)，继续

    return Ready { path, version, source }
record all → NeedManual { attempts }
```

**串行而非并发**：Windows Defender 会逐个扫描新启动的进程，并发 8 个 spawn 的总耗时反而比串行长 2–3 倍。

**时机**：不在启动时探测，在首次需要 Node 时惰性触发，`spawn_blocking` 内执行，不阻塞 UI 线程。

**缓存**：`{ path, version, mtime, size, checkedAt }` 写配置。下次启动只做 `exists` + `mtime` 比对，约 100ms。缓存失效时**静默重探**，只有全部失败才弹窗——用户卸载 nvm 换版本很常见，每次弹窗会非常烦。

## 5. 手动兜底

```
┌─ 未找到可用的 Node.js ─────────────────────────────┐
│                                                     │
│  已检查以下位置：                                    │
│   ✗ C:\Program Files\nodejs\node.exe          未找到 │
│   ✗ ...\nvm\v20.11.0\node.exe             版本过低  │
│   ✗ ...\Microsoft\WindowsApps\node.exe   不是有效程序│
│                                                     │
│  Node.js 用于 MCP 服务与数据库连接等可选功能。        │
│  不安装也可以使用编辑器、文件树和 Git。               │
│                                                     │
│  [ 选择 node.exe… ]  [ 下载便携版 (v22) ]           │
│  [ 暂不安装 ]                                        │
└─────────────────────────────────────────────────────┘
```

- 文件选择走 Tauri 原生对话框，过滤 `node.exe`
- 选中后**立即在同一弹窗内校验**，成功显示 `Node.js v24.15.0 — 可以使用`，失败显示原始 stderr
- 校验通过写 `{ path, version, source: "manual", resolvedAt }`
- **永不写回系统 PATH**、不修改 `.bashrc` / 注册表

### 便携版下载

- 源：`https://nodejs.org/dist/v22.x.y/node-v22.x.y-win-x64.zip`
- **校验 SHASUMS256.txt**，不匹配则删除并报错
- 解压到 `PathProvider::data_dir()/runtime/node/`
- 走同一 `validate()` 函数，成功后写入同一配置
- 镜像地址可配置（设置项），默认官方源

这条让"不捆绑"承诺成立（安装包不变大），同时消除"用户环境不可控"这个最大支持负担。

## 6. 进程注入

所有 Node 调用必须经过唯一出口：

```rust
impl NodeProcess {
    pub fn build(runtime: &NodeRuntime) -> Command {
        let mut cmd = Command::new(&runtime.path);   // argv[0] 是绝对路径
        cmd.env_remove("NODE_OPTIONS")               // 防注入：--require hook
           .env_remove("NODE_PATH")                  // 防模块解析污染
           .env("NO_COLOR", "1")
           .env("FORCE_COLOR", "0")
           .env("NODE_NO_WARNINGS", "1");

        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);             // CREATE_NO_WINDOW
        cmd
    }
}
```

`NODE_OPTIONS` 是真实攻击面：用户环境中若有 `--require some-hook.js`，sidecar 就在执行别人的代码。保留 `PATH`（子进程需要找依赖），但**绝不用它找 node**。

## 7. 安全红线

> **IPC 不得提供"传任意脚本路径 + 任意可执行文件"的通用运行命令。**

UI 层任何 XSS 就会升级为本机任意代码执行。正确做法：

- sidecar 只能从应用资源目录内的**构建期生成的清单**按 id 启动，清单带哈希
- 用户自定义 MCP server / DB 连接是显式授权行为：UI 展示完整命令行与路径，确认后才记录授权
- Tauri capabilities 中不出现任何动态路径；路径只存在于 Rust 运行时状态

## 8. Shell 探测（PowerShell 7 / bash）

与 Node 探测共用 `EnvSnapshot` 与 `DirLister`，但**语义不同**：Node 缺失可降级，shell 缺失则终端与 Agent 执行不可用，必须走引导安装。状态机保持独立，互不影响。

### 8.1 Windows：PowerShell 7

```
1. %ProgramFiles%\PowerShell\7\pwsh.exe
2. %ProgramFiles%\PowerShell\7-preview\pwsh.exe
3. where pwsh → 取第一条
4. %LOCALAPPDATA%\Microsoft\WindowsApps\pwsh.exe（Store 版，同样需排除别名桩）
```

校验：

```
spawn(pwsh, ["-NoLogo","-NoProfile","-NonInteractive","-Command",
             "$PSVersionTable.PSVersion.Major"], timeout=3s)
major >= 7 → Ready
major <  7 → TooOld（理论上不会出现，pwsh 不存在 5.x）
执行失败   → Broken
```

`pwsh` 与 `powershell` 是两个不同的可执行文件，**必须按文件名精确匹配**：

| 名称 | 含义 |
|---|---|
| `pwsh.exe` | PowerShell 7+，目标 shell |
| `powershell.exe` | Windows PowerShell 5.1，仅作显式降级选项 |

### 8.2 Linux：bash

```
1. /bin/bash
2. /usr/bin/bash
3. where bash
校验：bash --version → 解析版本号
```

bash 在现代发行版中几乎不会缺失（`/bin/sh` 可能是 dash，因此必须精确找 bash，不能用 `sh`）。

### 8.3 缺失引导

| 平台 | 行为 |
|---|---|
| Windows | 弹出引导卡，提供 winget 命令、复制、直接安装、打开下载页；允许"本次使用 5.1"的显式降级 |
| Linux | 提示安装 bash，列出已检测到的可用 shell（sh / zsh / fish），但不自动替换 |

**`[直接安装]` 的实现约束**：

```rust
// 在可见的 PTY 中执行，继承用户界面，让 UAC 与进度可见
let mut cmd = Command::new("winget");
cmd.args(["install", "--id", "Microsoft.PowerShell", "-e",
          "--source", "winget", "--accept-package-agreements",
          "--accept-source-agreements"]);
// 不 CREATE_NO_WINDOW —— 用户必须看到发生了什么
```

安装返回后**重新探测并校验版本**，不假定成功。`winget` 不存在时不显示该按钮。

### 8.4 与 Node 探测的区别

| 维度 | Node | Shell |
|---|---|---|
| 缺失后果 | 部分功能降级 | 终端与命令执行不可用 |
| 触发时机 | 惰性，首次需要时 | 启动时（终端是核心功能） |
| 缓存 | 需要，探测慢 | 不需要，单次 spawn 即可 |
| 版本门槛 | ≥22 推荐，低版本仅标注 | ≥7 硬性要求（PS 5.1 需显式选择） |
| 写入方式 | 允许用户手动指定路径 | 不允许手动指定，只允许安装或显式降级 |

Shell 不提供"手动填路径"——用户不会把 pwsh 装到奇怪位置，提供这个选项只会增加支持负担。

## 9. 测试

探测逻辑写成**纯函数**，由 `EnvSnapshot` 与 `DirLister` trait 驱动，不碰真实文件系统：```rust
#[test]
fn nvm_highest_version_wins() {
    let env = EnvSnapshot::builder()
        .var("APPDATA", r"C:\Users\t\AppData\Roaming")
        .file(r"C:\Users\t\AppData\Roaming\nvm\v20.11.0\node.exe", v(20, 11, 0))
        .file(r"C:\Users\t\AppData\Roaming\nvm\v22.4.0\node.exe", v(22, 4, 0))
        .build();
    let r = resolve(&env);
    assert_eq!(r.version, v(22, 4, 0));
}
```

必测场景矩阵：

| 场景 | 预期 |
|---|---|
| 只有 Store 别名桩 | `NeedManual`，标记 StoreAlias |
| nvm 多版本 | 取最高 |
| nvm 当前未 use | 仍能通过目录扫描找到 |
| PATH 陈旧（注册表与进程不一致） | 以注册表为准 |
| 版本 16 | `Ready/Legacy`，不阻断 |
| exe 存在但自检失败 | 跳过，尝试下一个 |
| 全部失败 | `NeedManual` |
| 手动路径校验通过 | 写配置，`Ready/Manual` |
| 便携版下载后校验 | 走同一 validate |
