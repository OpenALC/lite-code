# Agent Harness

## 0. 定位：harness 是我们的，不是别人的

传统做法是给 Claude Code / Codex 套一个 GUI。本项目不走这条路，原因很实际：

| | 套壳方案 | 本项目 |
|---|---|---|
| 运行时 | 依赖上游的 Bun / Node 版本 | 原生 Rust，零运行时依赖 |
| 启动 | Electron/Node 冷启动 3–6s | < 1s |
| 常驻内存 | 每套 200–300MB | 与主进程共享，< 60MB |
| 上游变更 | 输出格式一改就崩 | 契约自持 |
| 能力边界 | 受限于上游 CLI 的参数面 | 完全可控 |

因此 **harness 的核心循环在 Rust 中实现**。Node 只出现在可选集成里（MCP server 子进程、DB sidecar），且不可用时应用仍能工作。

## 1. 分层

```
apps/web ──── 会话 UI / 工具调用可视化 / 权限确认
   │  lc-proto (Channel 流式)
   ▼
lc-core::agent
   ├─ session    会话状态机、消息历史、上下文预算
   ├─ loop       LLM ↔ 工具 循环
   ├─ tools      工具注册表、执行、并发控制
   ├─ policy     权限模型、审批队列
   ├─ compact    上下文压缩（摘要 / 裁剪 / 外部化）
   └─ memory     长期记忆读写（Claude Code / Codex 适配）
lc-core::llm     provider 归一化 + 原始透传
lc-core::mcp     MCP Host（stdio / HTTP）
```

## 2. 会话循环

```
用户输入
  → 组装 system prompt（项目记忆 + 工具清单 + 环境快照）
  → 上下文预算检查 → 超出则 compact
  → 调 LLM（流式）
  → 若返回 tool_use：
       ├ 权限检查 → 需审批则挂起，等待 UI 确认
       ├ 执行（可并发，受 policy 限制）
       └ 结果回填 → 继续循环
  → 若无 tool_use：输出最终回复，结束
```

实现要点：

- **一切状态在 Rust 侧**。前端是显示器，窗口销毁后会话继续跑，重建后从 `session://state` 重放。
- **流式用 `Channel`** 而非全局 event。高频 token 流走 event 会拖垮事件总线。
- **取消**：每个请求带 `request_id`，取消通过 `CancellationToken` 传播到 HTTP 连接和工具执行。
- **循环上限**：默认 25 轮，可配置。超限时要求模型给出阶段性总结，而不是静默中止。

## 3. LLM 网关

不做重型 SDK，Rust 侧薄封装：`reqwest` + SSE 解析。

**端点范围（已定）**：v1 只做 **OpenAI Responses API**，v1.x 增加 **Anthropic Messages API**。不实现其他 provider 的适配层——OpenAI 兼容端点（DeepSeek、Moonshot、本地 Ollama 等）通过"改 base_url + 透传"覆盖，不单独写代码。

双模式并存：

| 模式 | 用途 |
|---|---|
| **归一化** | 统一 `messages / tools / stream`，跨 provider 一致体验 |
| **透传** | 允许直接给原始 JSON body 与额外 header |

透传是"发挥全部能力"的关键：prompt caching、thinking budget、服务商私有字段、新发布的参数，都不需要等我们跟版本。

流式事件流：

```rust
enum LlmEvent {
    TextDelta { index: u32, text: String },
    ThinkingDelta { text: String },
    ToolUseStart { id: String, name: String },
    ToolUseArgsDelta { id: String, json: String },
    Usage { input: u32, output: u32, cached: u32 },
    Done { stop_reason: StopReason },
    Error { code: ErrorCode, message: String },
}
```

密钥优先 `apiKey`（供应商管理页填写的本地明文，Unix 文件权限 0600），其次 `apiKeyRef`（`env:NAME`）；运行时 `LC_API_KEY` 环境变量优先级最高。SecretStore 尚未落地，接入后把本地明文迁移过去。**日志默认不记录 prompt / 响应内容**，只记模型、token 数、耗时、工具名。开源项目不做这条，issue 区迟早出现带密钥的日志。

### 3.1 供应商与模型配置

供应商管理是**左侧栏的一等功能**，不是设置页里的附属项。配置存在用户级 `providers.json`；预览版允许把 API Key 写入其中的 `apiKey` 字段（Unix 0600，Windows 依赖用户目录 ACL），SecretStore 落地后再迁移。

```jsonc
{
  "providers": [
    {
      "id": "openai",
      "kind": "openai-responses",        // 端点类型，决定协议实现
      "label": "OpenAI",
      "baseUrl": "https://api.openai.com/v1",
      "apiKey": "sk-...",              // 可选：本地明文密钥（0600）
      "apiKeyRef": "env:OPENAI_API_KEY", // 可选：环境变量引用
      "enabled": true
    }
  ],
  "models": [
    {
      "id": "gpt-5",                     // 显示 id：UI 上看到的名字
      "provider": "openai",
      "modelId": "gpt-5-2025-08-07",     // 实际 id：发请求时用的值
      "thinking": ["off", "low", "medium", "high"],
      "contextK": 400,                   // 上下文窗口，单位 k
      "compressAtK": 300,                // 压缩阈值，单位 k
      "capabilities": ["tools", "vision", "reasoning"]
    }
  ],
  "defaultModel": "gpt-5",               // 用于压缩与风险判定
  "defaultThinking": "medium",
  "subagentModel": "gpt-5-mini",         // 子 Agent 模型（当前仅持久化）
  "subagentThinking": "low"
}
```

四组字段的含义与约束：

| 字段 | 含义 | 约束 |
|---|---|---|
| `id` | 显示 id | 唯一，可自由改名（"我的便宜模型"），仅本地生效 |
| `subagentModel` | 子 Agent 模型 | 必须是已配置的模型 id；当前仅持久化，尚未接入 subagent 执行 |
| `subagentThinking` | 子 Agent 思考挡位 | 当前仅持久化；UI 从所选模型的 `thinking` 列表提供选项 |
| `modelId` | 实际 id | 发往 API 的原值，不校验格式——服务商可能随时加新模型 |
| `thinking` | 支持挡位 | 至少一项；不含 `off` 的模型，UI 隐藏关闭选项 |
| `contextK` | 上下文窗口 | > 0；仅影响预算计算，不向上游校验 |
| `compressAtK` | 压缩阈值 | 必须 < `contextK`；建议 0.7–0.85 倍 |

**显示 id 与实际 id 分离**的价值：同一模型挂不同 baseUrl（官方 / 中转 / 本地），或同一个服务商跑同一模型的两个版本做对比，都能在 UI 上区分，而不必改请求参数。

**默认模型**是独立概念，用于两件"不该花主力模型"的事：

1. **上下文压缩**（§6）——用便宜模型总结历史，成本可降一个数量级
2. **auto 模式的风险判定**（§5.2）——延迟敏感，要求快

因此默认模型建议单独配置为一个低成本、高吞吐的模型，并在 UI 上标注其用途，避免用户误以为它只是"没选模型时的兜底"。

配置变更的生效时机：模型参数（思考挡位、上下文）在**下一次请求**生效；供应商启用/禁用立即生效；删除仍被会话引用的模型时，要求先切换会话模型。

### 3.2 上下文窗口与压缩

每个会话的可用预算 = `contextK`，用量条实时显示 `已用 / contextK`，并在 `compressAtK` 处画一条阈值刻度线。

跨越阈值时触发压缩，顺序见 §6。压缩本身也消耗 token，因此阈值不能贴得太近上限——留 15%–30% 余量。

## 4. 工具注册表

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    fn risk(&self) -> Risk;                     // Read | Write | Execute | Network
    fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput>;
}
```

内置工具：

| 类别 | 工具 |
|---|---|
| 文件 | `read_file` `write_file` `edit_file` `glob` `grep` |
| 执行 | `shell`（受 policy 约束，非 PTY） |
| 检索 | `search_code`（Rust 侧直接 WalkDir + 正则，不依赖外部） |
| 编排 | `todo_write` `task`（子会话） |
| 集成 | `git_*`、`ssh_exec`、`db_query`（复用 L1 服务） |
| 外部 | MCP tools（动态注册） |

**并发控制**：只读工具可并行，写类工具串行且加路径锁，避免同一文件被两个工具同时改。

### 4.1 环境契约（Environment Contract）

模型最常见的失败模式不是能力不足，而是**假设了错误的 shell 方言**：在 PowerShell 里写 `grep -r`、在 bash 里写 `Get-ChildItem`、用 cmDer 的 `&&` 语义串命令。因此每个会话在 system prompt 中**显式声明执行环境**，而不是让模型推断。

契约在会话创建时由 Rust 侧生成，随 system prompt 一并下发，且**不可被用户消息覆盖**：

```xml
<environment>
platform: windows
os_version: "Windows 11 Pro 10.0.26200"
arch: x86_64
shell: powershell
shell_version: "7.4.6"
shell_path: "C:\\Program Files\\PowerShell\\7\\pwsh.exe"
shell_dialect: pwsh
path_separator: "\\"
command_separator: ";"
case_sensitive_paths: false
default_encoding: utf-8
home: "C:\\Users\\Yang"
workspace_root: "D:\\code\\lite-code"
temp_dir: "C:\\Users\\Yang\\AppData\\Local\\Temp"
available: [git, node, npm, cargo, rg]
</environment>
```

必填字段及其作用：

| 字段 | 作用 |
|---|---|
| `platform` / `arch` | 决定路径分隔符、可执行后缀、包管理器 |
| `shell` / `shell_dialect` | **最关键**。模型据此选择语法 |
| `command_separator` | `;`（PowerShell）/ `&&`（bash 4+） |
| `case_sensitive_paths` | Windows 不区分，Linux 区分 |
| `default_encoding` | 决定是否需要处理 GBK / UTF-16 输出 |
| `available` | 已探测到的外部工具，避免模型调用不存在的命令 |

### 4.2 方言约束提示词

契约之后紧跟一段**方言规则**，按 shell 切换。这段是硬约束，不是建议：

**PowerShell 7（Windows）**

```
当前 shell 是 PowerShell 7（pwsh），不是 cmd.exe，也不是 bash。

规则：
- 用 PowerShell 语法：Get-ChildItem / Get-Content / Select-String / Where-Object
- 不要用：grep、sed、awk、ls -la、cat、rm -rf、export、$VAR 形式的 POSIX 环境变量
- 管道传对象，不是文本。Get-ChildItem | Where-Object { $_.Length -gt 1MB }
- 命令串联用 ; 或 if ($?) {}，不要用 && （PS7 虽支持，但为兼容降级模式统一禁用）
- 路径用 \ 或 / 均可，含空格的路径必须加引号
- 环境变量：$env:NAME，不是 $NAME
- 删除用 Remove-Item -Recurse -Force，不要 rm -rf
- 查找文件用 Get-ChildItem -Recurse -Filter，或直接用 glob 工具
- 非交互执行，禁止调用 Read-Host、Pause、more 等会阻塞的命令
- 不要启动交互式子 shell（不要执行 powershell、pwsh、cmd、bash）
```

**bash（Linux）**

```
当前 shell 是 bash（非交互模式，未加载 .bashrc）。

规则：
- 标准 POSIX/bash 语法
- 命令串联用 && 或 ;；需要错误中断时用 set -e
- 环境变量：$NAME 或 ${NAME}
- 路径分隔符 /
- 优先使用 rg / fd，若不可用再用 grep -r / find
- 非交互执行，禁止 vim、less、top 等全屏程序
- 不要启动交互式子 shell
```

### 4.3 为什么不用"自动转换"

有一个诱人的方案：让模型写 POSIX 命令，Rust 侧自动翻译成 PowerShell。**不采用**，原因有三：

1. 翻译层本身就是 bug 来源，出错时用户看到的命令和实际执行的命令不一致，无法排障。
2. 命令必须是**可审计、可复制**的——用户在权限卡上看到的字符串就该是真正执行的那一条。
3. 模型对目标方言直接生成的成功率，高于"生成 + 翻译"两段式。

同理，也**不提供 `cmd.exe` 兼容层**。单一 shell 原则的收益远大于多方言兼容的成本。

### 4.4 版本与能力声明

`available` 列表不是装饰。典型用法：

- 有 `rg` → 提示"优先用 rg，比 grep 快且尊重 .gitignore"
- 无 `git` → 提示"不要尝试 git 命令，改用文件工具"
- `node` 版本 < 22 → 契约里标 `node_status: unverified`，提示"避免依赖 node:sqlite 等新 API"

`available` 由 RUNTIME.md 的探测结果填充，与用户看到的状态栏信息同源——**UI 展示什么，模型就看到什么**，避免两边判断不一致。

## 5. 执行模式与权限

### 5.1 四种模式

模式是**策略预设**，不是提示词建议。切换模式只改变 `PolicySet`，执行闸门始终在 Rust 侧。

| 模式 | id | 工具可用性 | 审批规则 | 典型场景 |
|---|---|---|---|---|
| 计划模式 | `plan` | 仅只读工具 | 不执行任何写/执行类工具，产出计划文本 | 接手陌生代码库，先摸清结构 |
| 仅对话 | `chat` | 无工具 | 不调用任何工具 | 问概念、写文案、review 思路 |
| 自主行动 | `auto` | 全部 | 静态黑名单 ∪ 用户黑名单 ∪ 模型判定为高风险 → 询问 | 日常默认 |
| 完全自主 | `yolo` | 全部 | 静态黑名单 ∪ 用户黑名单 → 询问；其余自动执行 | 信任度高的重复性任务 |

模式在**会话级**生效，另有**全局默认模式**（设置项）。优先级：会话覆盖 > 全局默认 > `auto`（硬编码兜底）。UI 上常驻可见，切换会写入会话记录（审计用）。运行中切换需确认，且不追溯已批准的命令。

**`yolo` 的确认时机**：每会话首次切换需确认，且**每次应用启动重新确认**一次。后者是有意的——长期挂着的应用如果一直保持完全自主，"我忘了它还开着"是最常见的事故来源。

### 5.1b 子代理

子代理（subagent）是 harness 的核心能力之一，用于把可并行的探查、检索、批量修改分派出去，避免主会话上下文被中间过程污染。

```jsonc
{
  "subagents": [
    {
      "id": "explorer",
      "label": "代码探查",
      "model": "gpt-5-mini",          // 可不同于主会话模型
      "thinking": "low",              // 子代理独立思考强度
      "mode": "plan",                 // 子代理永远是只读模式
      "maxTurns": 12,
      "systemPrompt": "你是代码探查代理。只读，不修改文件，返回结构化摘要。"
    },
    {
      "id": "fixer",
      "label": "批量修改",
      "model": "gpt-5",
      "thinking": "high",
      "mode": "auto",
      "maxTurns": 20
    }
  ],
  "defaultSubagent": "explorer"
}
```

设计要点：

- **子代理不能提权**。子代理的模式只能独立于主会话，但**静态黑名单永远继承**；子代理的 `mode` 若为 `yolo`，仍受同一黑名单约束，且审批卡冒泡到主会话 UI，不在子会话内静默通过。
- **思考强度独立**。探查类子代理用 `low` 省钱，修改类用 `high` 求稳——这是成本控制的主要手段。
- **并发上限**。默认同时最多 4 个子代理，超出排队；子代理本身不能再派生子代理（深度固定为 1），避免递归失控。
- **上下文隔离**。子代理只看到主会话传入的**任务描述 + 必要文件切片**，不继承完整历史。返回值有大小上限（默认 8k token），超出截断并附摘要。
- **可观测**。子代理在 UI 上表现为一个可展开的块，显示模型、思考强度、轮次进度、累计 token，而不是黑盒。

### 5.1c 模型能力探测

"通过模型 id 直接拿到参数"这件事，各端点的能力差异很大：

| 端点 | 可获取 | 说明 |
|---|---|---|
| OpenAI Responses | `/v1/models` 返回 id、created、owned_by | **不含**上下文窗口、价格、思考挡位 |
| Anthropic Messages | 官方文档有完整表格，但**无 API** | 只能内置对照表 |
| 第三方 / 自建 | 视实现而定，多数只返回 id | 不可依赖 |

结论：**不存在通用的"id → 参数"接口**。因此设计成三层，逐层降级：

```
① 探测请求：GET {baseUrl}/models
      → 拿到模型 id 列表，用于下拉选择与校验 id 是否有效
② 本地映射表：内置已知模型的能力表（context / thinking / capabilities）
      → 命中则预填，并标注「来自内置表，可能过期」
③ 手工覆盖：用户在供应商面板里填写的值
      → 优先级最高，且永不被 ①② 覆盖
```

UI 上的呈现规则：

- 预填值带一个浅色来源标记（`推测` / `内置` / `手动`），三者视觉可区分
- 预填值可编辑，一旦编辑立即变 `手动` 并持久化
- 内置表随版本更新，但**不自动改用户已手动设置的值**
- 拿不到任何信息时，`contextK` 留空并**阻止启用该模型**——上下文窗口是预算计算的前提，不能猜

内置映射表的结构（随二进制发布，不是配置文件）：

```jsonc
// model-catalog.json，打包进二进制
{
  "gpt-5":       { "contextK": 400, "thinking": ["off","low","medium","high"], "capabilities": ["tools","vision","reasoning"] },
  "gpt-5-mini":  { "contextK": 128, "thinking": ["off","low","medium"],        "capabilities": ["tools","vision","reasoning"] }
}
```

维护成本说明：这张表会随服务商更新而过期，因此每条命中都标注"可能过期"，且提供 `[刷新]` 调用 ① 重新拉取 id 列表。**不要试图让它总是准确**——只要保证填错时用户能改、且界面诚实地标明来源即可。

### 5.2 三层判定顺序

```
命令 → ① 静态黑名单（内置，不可配置、不可清空）
     → ② 用户黑名单（用户自定义正则，只能追加）
     → ③ 模型风险判定（仅 auto 模式启用，只能收窄不能放宽）
     → 执行 / 询问
```

**① 静态黑名单**（永远生效，任何模式都不能绕过）：

```
rm -rf /            rm -rf ~          mkfs*           dd of=/dev/*
git push --force    git reset --hard  git clean -fdx
DROP DATABASE       TRUNCATE         shutdown /s
reg delete          format            Remove-Item -Recurse -Force C:\
curl ... | sh       iwr ... | iex     chmod 777 /
```

这些项**硬编码在二进制里**，不读配置文件。理由：用户被诱导改配置时，至少还有一层兜底。

**② 用户黑名单**：正则或 glob，存在项目级配置里，默认给一组建议项（`git push`、`npm publish`、`ssh *`、`db_*`），用户可增删。

**③ 模型风险判定**：仅在 `auto` 模式下调用。实现上是一次轻量 LLM 调用（可用低成本模型），输入是**将要执行的完整命令 + 工作目录 + 模式**，输出 `{ risk: low|high, reason }`。

关键约束：

- 判定结果**只能把 low 升级为 high**，不能把 high 降为 low
- 判定调用失败 / 超时（>2s）→ **按 high 处理**（fail-closed）
- 判定输入不包含模型自己的历史输出，避免自我说服
- 判定结果和理由写入会话记录，用户可事后审计

`yolo` 模式不调用模型判定——这个模式的定义就是"不问模型，只问黑名单"。

### 5.3 工具级风险映射

每个工具声明静态风险等级，作为模型判定的先验：

| 风险 | 工具 | auto 模式行为 |
|---|---|---|
| `Read` | `read_file` `glob` `grep` `search_code` | 自动允许 |
| `Write` | `write_file` `edit_file` | 项目内自动；项目外询问 |
| `Execute` | `shell` `ssh_exec` | 走三层判定 |
| `Network` | `http` `db_query` `mcp_*` | 询问（`yolo` 下自动） |

### 5.4 审批 UI 约定

审批卡必须展示：**完整命令原文**、工作目录、触发来源（静态黑名单 / 用户规则 / 模型判定 + 理由）、以及影响范围摘要（如"将修改 3 个文件"）。

按钮：`允许` / `本次会话允许同类` / `拒绝`。第三条恒存在，且默认聚焦。

`yolo` 模式下仍会出现审批卡（来自黑名单命中），此时卡片头部必须明确标注"完全自主模式下仍被拦截"，避免用户误以为模式没生效。

### 5.5 思考强度

思考强度是**模型维度**的属性，不是会话维度：

- 每个模型在供应商配置里声明支持的挡位（`off / low / medium / high`，或自定义）
- 会话级选择挡位，但只能在该模型支持的范围内选
- 映射关系按 provider 在 Rust 侧实现（OpenAI Responses 走 `reasoning.effort`，Anthropic 走 thinking budget token）
- 切换模型时，若原挡位不被支持，回落到该模型的中档并提示一次
- `off` 不是"不思考"，而是"不显式声明"——部分模型默认就带思考

## 6. 上下文管理

预算分配（默认，按模型上下文长度缩放）：

```
system + 工具定义   ~15%
项目记忆            ~10%
历史消息            ~50%
工具结果            ~20%
预留输出            ~5%
```

超出时的压缩顺序：

1. 丢弃已过期的工具结果（保留摘要行）
2. 大文件读取结果外部化（存文件，上下文只留路径 + 前 200 行）
3. 早期对话摘要为一条系统消息
4. 仍超出则提示用户新建会话

**不要用"静默截断"**——模型会基于残缺上下文编造。每次压缩都在 UI 上留一条可展开的记录。

## 7. 长期记忆适配

数据表驱动，加一个 Agent 只是加一张表。

| Agent | 全局 | 项目 | 配置 |
|---|---|---|---|
| Claude Code | `~/.claude/CLAUDE.md` | `<root>/CLAUDE.md`、`.claude/**` | `~/.claude.json`、`.mcp.json`、`.claude/settings.json` |
| Codex | `~/.codex/AGENTS.md` | `<root>/AGENTS.md` | `~/.codex/config.toml` |
| Lite Code | `~/.lite-code/memory.md` | `<root>/.lite-code/memory.md` | `~/.lite-code/config.json` |

```rust
pub trait AgentAdapter {
    fn id(&self) -> &str;
    fn detect(&self, env: &EnvSnapshot) -> Option<Installation>;
    fn memory_files(&self, root: &Path) -> Vec<MemoryFile>;
    fn read(&self, f: &MemoryFile) -> Result<String>;
    fn write(&self, f: &MemoryFile, content: &str) -> Result<()>;
}
```

**写入必须保格式**：

- TOML 用 `toml_edit`（保注释、保顺序）
- JSON 用最小 diff 补丁，禁止 `serde_json` 读入后整体重写
- 写入前备份为 `.bak`，原子替换（临时文件 + rename）

用户手写的注释和顺序一夜之间消失，是不可接受的破坏性操作。这条在开源项目里会被当成 bug 报上来。

**读取语义**：桌面端与 Agent 共享同一份记忆文件，读写都要处理并发（文件锁 + mtime 检查），冲突时以磁盘为准并提示重新加载。

## 8. MCP

实现为 **MCP Host（客户端）**，支持 `stdio` 与 `HTTP/SSE`。

- 配置读写复用上面的"保格式编辑"
- stdio server 启动：Windows 上 `npx` 是 `.cmd`，`Command` 直接调用会失败。可靠做法是**解析出 npm-cli.js 绝对路径，用 node 直接跑**，绕开 cmd.exe 转义
- 每个 server 独立子进程，崩溃不影响主进程，自动重连带退避
- 工具名冲突加命名空间 `server_name/tool_name`
- UI 展示每个 server 的连接状态、工具数量、最近错误

## 9. L2：外部 CLI 会话宿主（v1.1）

用同一套 PTY 基础设施承载 `claude` / `codex` 的可交互会话，**不做输出解析依赖**：

- 优先消费结构化输出模式（如 `--output-format stream-json`）
- 无结构化模式时退回纯文本渲染，不做语义解析
- 每标签一个 PTY + 一个进程组，状态在 Rust 侧
- 这样即使上游改格式，最坏情况是显示变朴素，而不是崩溃

架构上现在只需预留：`lc-core::pty` 不绑定终端 UI，`ProcessSupervisor` 支持长驻会话。增量成本接近于零。

## 10. 自测

harness 的测试不依赖真实模型：

- **录制回放**：把一次真实会话的 SSE 流录成 fixture，回放驱动循环，断言工具调用顺序、压缩触发点、取消行为
- **假 provider**：一个本地 HTTP server，按脚本返回预设响应，用于并发与错误路径
- **工具单测**：每个工具独立测，覆盖路径越界、超大输出、编码异常
