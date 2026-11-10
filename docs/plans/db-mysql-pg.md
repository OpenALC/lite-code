# db lane：MySQL / PostgreSQL 支持实施设计

> 状态：设计定稿，待执行（preview.65 基线）。
> 范围：MySQL / PostgreSQL 端到端可用（SQL Server 随同一 bridge 已打包，跟随复用，不单独设计）。
> 前提校正：**bridge 执行层与契约已落地**，本文档覆盖剩余切片（UI 表单、CI 服务容器、字节截断加固）。

## 0. 结论（先行）

- **推荐方案：A（Node db-bridge sidecar）**——且该架构已落地，无需返工；本文档是"补齐计划"而非"选型计划"。
- **剩余工作三件套**：`DbPanel.vue` 远程连接表单（唯一阻断端到端的缺口）、结果集字节级截断对齐 `MAX_STREAM_BYTES`、GitHub Actions 引入 PG/MySQL 服务容器的集成测试。
- **最大风险**：Node 缺失时的降级体验、Windows 环境服务连通性（内网/SSL/自定义端口）、凭证处理（`keyring://` 尚未实现，只有 `env:`）。

## 1. 现状基线（与任务前提的差异）

任务前提"数据库 lane 只实现了 SQLite"**不准确**。证据：

| 层 | 位置 | 状态 |
|---|---|---|
| bridge 驱动 | `apps/db-bridge/src/drivers.mjs:1-271`（pg/mysql2/mssql 适配器）、`config.mjs:112-220`（连接参数映射）、`config.mjs:222-276`（方言 schema/explain SQL） | 已落地 |
| bundle 分发 | `apps/db-bridge/scripts/build.mjs:16-42` esbuild 单文件 → `crates/lc-core/assets/db-bridge.mjs`，`sidecar.rs:53` `include_str!` 安装，CI `build:check` 校验同步（`ci.yml:216-233`） | 已落地 |
| Rust sidecar | `crates/lc-core/src/db/sidecar.rs`（spawn/握手/超时/kinds 声明/错误映射，全部按 4 驱动实现） | 已落地 |
| SQL 闸门 | `crates/lc-core/src/db/sql.rs:46-65` `SqlDialect` 四方言（MySQL 反斜杠转义、PG dollar quote 均已支持） | 已落地 |
| 契约 | `crates/lc-proto/src/db.rs:10-15` `DbKind` 四变体；`command.rs:40-48` 9 个 `db_*` 命令；`event.rs:66` `db:status`；`db_*` 错误码 | 已落地 |
| 单测 | `apps/db-bridge/test/bridge.test.mjs`（fake driver 全路径）、`bundle.test.mjs`（真 bundle 冒烟）、Rust 侧 SQLite roundtrip | 已落地 |
| **UI** | `apps/web/src/components/DbPanel.vue:97-99` 非 SQLite 一律拒绝、`saveForm` 硬编码 `kind: 'sqlite'`、`dbStore.ts:79-81` `canConnect` 限 SQLite、`locales/zh-CN.json:376` "仅 SQLite 已实现" | **缺口** |
| **字节截断** | `apps/db-bridge/src/format.mjs:17-36` `cell()` 无字节上限；行数封顶 1000（`format.mjs:6-7`）但 1000 行 × 大 TEXT 可撑爆 IPC/内存 | **缺口** |
| **CI 集成测试** | `ci.yml` 无 PG/MySQL 服务容器，远程驱动只有 fake 单测 | **缺口** |
| **keyring** | `config.mjs:102-105` `keyring://` 显式拒绝（待 SecretStore） | 已知限制 |

## 2. 方案对比

### A. Node db-bridge sidecar（推荐）

- **复用先例**：ext-host（`sidecars/ext-host/src/host.mjs`）已验证"Rust 进程管道 + Node stdio JSON-RPC"模式；db-bridge 同构（`sidecar.rs:140-191` 启动、`index.mjs:1-105` NDJSON 循环）。
- **依赖分发**：`pg`/`mysql2` 由 esbuild 预打包进单文件 `db-bridge.mjs`（约 2 MiB 级），随 `include_str!` 进二进制，**运行时不需要 node_modules、不需要联网惰性安装**；不存在"离线装不了驱动"问题。驱动懒加载（`drivers.mjs:13-26`），未打包的 kind 在 `initialize` 的 `kinds` 里不声明，connect 直接 `db_unavailable`。
- **离线降级**：Node 缺失 → `DbService::load_default` 照常工作，connect 时才报 `DbUnavailable`（`service.rs:179-188`、`service.rs:853-888` 有单测）；list/save/delete 全程可用。
- **成本**：bundle 体积 +2 MiB 级；进程管理代码已有。

### B. Rust 原生客户端 crate（不推荐）

- `tokio-postgres`/`mysql_async` 是 async 栈，workspace 是同步 std（ureq 风格），需内嵌 tokio current-thread 运行时 + `block_on`，违反"运行时不引入重型异步运行时"约束。
- 二进制体积显著上涨（rustls/openssl、tokio、驱动）；`ring`/`aws-lc-sys` 使 **arm64 交叉编译高危**（MILESTONES 有 Linux arm64 门禁）。
- 安装包内置 MySQL/PG 客户端库违反环境约束。否决。

### C. 外部命令行客户端（不推荐）

- `psql`/`mysql` CLI 在 Windows 开发机可用性不可控（很多机器未安装、不在 PATH）；输出格式解析脆弱；错误分类靠 stderr 文本匹配；无统一参数协议。
- 可作为未来"零 Node 环境"的备选降级路径记录，不作主线。否决。

**结论：维持方案 A**（已被仓库现状证实）。

## 3. 推荐方案详细设计

### 3.1 进程生命周期（对齐 `sidecar.rs` 现有约定）

- **谁启动**：`DbService::ensure_sidecar`（`service.rs:525-552`）——首个 connect 时惰性 spawn，一个 service 复用一个 sidecar 进程，进程内持多个数据库连接（`bridge.mjs:46-47` connections Map）。
- **安装**：`install_bridge`（`sidecar.rs:486-518`）内容比对避免重复写盘，临时文件 + rename，Windows 下 rename 不覆盖时先删旧文件；进程内 `INSTALL_LOCK` 防并发。
- **清理**：响应超时 → `terminate` kill 子进程（`sidecar.rs:425-438`）；`Drop`/`shutdown` 同路径；bridge 侧 stdin 关闭 → `closeAll` 关 socket（`index.mjs:96-100`）；SIGINT/SIGTERM 兜底（`index.mjs:102-105`）。Windows `CREATE_NO_WINDOW`（`sidecar.rs:154-158`）。
- **状态同步**：fatal 错误（进程退出/超时/协议错）→ `reset_sidecar` 整体丢弃，active 连接统一标 `Error`，下次 connect 重建（`service.rs:562-596`）；非致命（SQL 拒绝/只读/参数）不断连。
- **Node 探测**：`runtime::probe_real()` → `NodeStatus::Ready/NeedManual`（`service.rs:183-187`），不反向依赖其他 lane 内部路径。

### 3.2 协议：JSON-RPC 方法与 9 个 db 命令的映射

transport：stdin/stdout NDJSON JSON-RPC 2.0，请求串行处理（`index.mjs:74-94`）。握手 `initialize` → `protocolVersion: 1` + `kinds` 声明（`sidecar.rs:194-217`）。

| 契约命令（`command.rs:40-48`） | DbService 方法 | sidecar RPC | 说明 |
|---|---|---|---|
| `db_list` | `list`（纯本地） | — | 配置内存表 |
| `db_save_connection` | `save_connection` | 条件 `disconnect` | 配置变更时先断旧连接（`service.rs:218-259`） |
| `db_delete_connection` | `delete_connection` | `disconnect` | |
| `db_connect` | `connect` | `connect` | 参数：`connection_id/kind/host/port/database/user/secret_ref/read_only/extra` |
| `db_disconnect` | `disconnect` | `disconnect` | |
| `db_query` | `query`（先过 `SqlGuard`） | `query` | `{connection_id, sql(guarded), limit}` |
| `db_execute` | `execute`（先过闸门） | `execute` | |
| `db_schema` | `schema` | `schema` | 不执行用户 SQL，固定元数据查询 |
| `db_explain` | `explain`（先过闸门） | `explain` | PG/MySQL 为 `EXPLAIN <sql>` |
| `db:status` 事件 | 状态变更 → `CoreEvent` | — | 连接 state 变化下发 UI（`event.rs:66`） |

SQL 闸门在 Rust 侧执行层之前：分类（Read/Write/Ddl/Unknown fail-closed）、单语句、静态黑名单、无 WHERE 写拒绝、`LIMIT` 注入（`sql.rs`、`mod.rs:40-48` 按 kind 选方言）。

### 3.3 错误码映射

bridge 字符串码（`errors.mjs:12-15`）→ `map_bridge_error`（`sidecar.rs:612-633`）→ `DbServiceError`（`service.rs:81-99`）→ 契约 `ErrorCode`：

| bridge 码 | 语义 | 覆盖场景 |
|---|---|---|
| `db_unavailable` | 环境/连接层失败 | Node 缺失、驱动未打包、TCP 连接失败/超时、认证失败（PG 28P01 / MySQL 1045，消息经 `sanitize` 脱敏）、secret_ref 指向的 env 未设置、`keyring://` 未实现 |
| `db_rejected` | 数据库拒绝执行 | SQL 语法错误、语句权限不足（PG 42501 / MySQL 1142）、驱动执行失败 |
| `db_read_only` | 只读连接写操作 | 闸门拒绝 + bridge 二次检查（`bridge.mjs:182-184`）+ PG 只读会话验证（`bridge.mjs:130-139`） |
| `invalid_argument` | 请求形态错误 | 缺字段、port 越界、`extra` 非法 JSON、未知 connection_id |
| （Rust 侧）`Timeout`/`Io`/`Protocol` | 本地等待/管道/帧错误 | 超时常量见 3.5；协议错误 → `Internal` |

驱动原始错误（SQLSTATE/errno）不新增契约码，由 bridge 归并到上述四类并在 message 保留原文（已脱敏）。

### 3.4 连接串安全（不落盘明文）

- `db.json` **无 password 字段**；`DbConnectionConfig.secret_ref` 只是引用（`db.rs:106-119`），有单测断言落盘内容不含密码（`service.rs:768`）。
- 密码仅由 bridge 从子进程环境变量解析（`config.mjs:84-106`）：`env:NAME` 为空 → `db_unavailable`（不编造密码）；`keyring://`/`vault://` 显式拒绝直到 SecretStore 落地。
- 错误消息脱敏：`sanitize(message, password)`（`errors.mjs:22-25`），connect 失败路径已测（`bridge.test.mjs:449-467`）。
- 子进程环境剥离：`scrub_secret_env`（`sidecar.rs:147`）防止 provider 密钥泄漏进 sidecar。
- `extra` JSON 先展开、身份字段后覆盖，extra 无法改写 host/user/password（`config.mjs:108-111`）；MySQL 强制 `multipleStatements: false`（`config.mjs:165-167`）。

### 3.5 超时与结果集截断

- **现有时延**（`sidecar.rs:36-48`）：INIT 10s / CONNECT 15s（bridge 内部 connectTimeout 10s，多 5s 给驱动清理）/ QUERY 30s / EXEC 30s / SCHEMA 30s / EXPLAIN 10s。超时即 kill 子进程。
- **行数**：默认 100、上限 1000（`format.mjs:6-7`、`service.rs:33-35`）；bridge 强制切片，SQL 自带更大 `LIMIT` 不能放大（`bridge.mjs:158-173`）；`truncated` 标记；Rust 侧二次切片防恶意/异常 sidecar（`sidecar.rs:657-666`）。
- **缺口与整改（M2 必做）**：`cell()` 无字节上限（`format.mjs:17-36`）。新增 `MAX_CELL_BYTES = 4096`（单元格截断加省略后缀）与 `MAX_RESULT_BYTES = 128 KiB`（对齐 `crates/lc-core/src/agent/shell.rs:32` `MAX_STREAM_BYTES` 的既有先例），bridge 在切片循环里累计字节预算、超限即 `truncated: true` 停取；pg/mysql 驱动侧用游标/流式（pg 的 `rowMode: 'array'` 已避免对象物化，逐行累计即可）。SQLite 路径（`iterate` 早停）同步加预算。

## 4. DbKind 扩展：契约变更点清单

| 变更点 | 状态 | 位置 |
|---|---|---|
| lc-proto `DbKind` 新变体 `Postgres`/`Mysql`（`Mssql`） | ✅ 已落地 | `crates/lc-proto/src/db.rs:10-15` |
| TS 绑定 codegen + `DB_KINDS` 守卫 | ✅ 已落地 | `apps/web/src/services/dbTypes.ts:34` |
| 9 个 `db_*` 命令 + `db:status` + `db_*` 错误码 | ✅ 已落地 | `command.rs:40-48`、`event.rs:66` |
| `validate_config` 远程校验（host 非空、port≠0） | ✅ 已落地 | `service.rs:606-632` |
| **DbPanel 连接表单：host/port/user/密码（env 变量名）/database/read_only 字段差异** | ⬜ 待做 | `DbPanel.vue:39-57` draft 扩展；远程 kind 渲染新模板（替换 `231-249` 的 sqlite-only 分支与 `:97-99`、`:254`、`:343` 的拦截） |
| **dbStore 放开 `canConnect` 的 kind 限制** | ⬜ 待做 | `dbStore.ts:79-81` |
| **i18n**：删/改 `db.sqliteOnly`，新增 host/port/user/passwordEnv/database 标签 | ⬜ 待做 | `zh-CN.json:376`、`en.json:376` |
| 表单校验（host 必填、port 1–65535、database 必填） | ⬜ 待做（UI 层，service 层已兜底） | `DbPanel.vue` `saveForm` |
| DbWorkbench 查询/执行路径 | 无需变更 | 契约不变 |

表单设计决策：**密码输入框只接受环境变量名**（如 `env:PGPASS` 中的 `PGPASS`），UI 不收集明文密码，与 `secret_ref` 契约一致；`user` 可空（PG 回落 OS 用户）。

## 5. 测试策略

### 5.1 无真实 PG/MySQL 的单测（现状模式，继续扩展）

- **fake driver 注入**：`createBridge({ loadDrivers })`（`bridge.mjs:42-45`）替换驱动模块，`bridge.test.mjs:96-285` 的 `pgModule`/`mysqlModule` 已覆盖参数映射、read-only 会话、schema SQL、explain、脱敏、limit/truncated。新增用例：字节截断（大 cell / 大结果）、SQLSTATE/errno → 四类错误归并表。
- **Rust 侧无 Node 依赖的协议测试**：帧格式、错误映射、kinds 声明（`sidecar.rs:884-945`、`1473-1511`）。
- **bundle 冒烟**：`bundle.test.mjs` spawn 真 bundle 验证 initialize/connect/query。

### 5.2 CI 服务容器（新增 job，M2 落地）

```yaml
# .github/workflows/ci.yml 新增 db-integration job（仅 ubuntu；Windows 无容器）
services:
  postgres: { image: postgres:16, env: POSTGRES_PASSWORD: test, ports: 5432, options: --health-cmd pg_isready }
  mysql: { image: mysql:8.0, env: MYSQL_ROOT_PASSWORD: test, ports: 3306, options: --health-cmd "mysqladmin ping" }
```

- 集成测试经 `LITE_CODE_TEST_NODE` + `PGPASS`/`MYSQL_PWD` env secret_ref 走完整 sidecar 管道；无服务时 `#[ignore]`/环境探测跳过（沿用 `test_node()` 模式，`sidecar.rs:816-831`）。
- **Windows runner 只跑 fake/降级路径**（Node 缺失 → `DbUnavailable`），真实服务集成只在 Linux job。
- 录制回放：fake driver 覆盖已足，不做。

## 6. 里程碑切片（每片独立可发布）

| 切片 | 内容 | 估点 | 独立发布条件 |
|---|---|---|---|
| **M1 连接+列表模式**（3 点） | DbPanel 远程表单（host/port/user/env 密码名/database/read_only）、`dbStore.canConnect` 放开、i18n、表单校验 | UI 可建/改/删 pg/mysql 连接，connect 成功并在列表展示 `db:status`；不依赖 M2 |
| **M2 查询/执行**（5 点） | schema 树渲染（PG schema 层级 / MySQL 扁平，桥接 `schemaTree` 已返回）、workbench 查询/执行/explain 打通远程连接、**字节截断加固**（3.5）、CI 服务容器集成测试 job | agent `db_query`/`db_execute` 对真实 PG/MySQL 闭环（CI 绿）；SQLite 行为零回归 |
| **M3 UI 完善**（3 点） | 连接错误分类提示（Node 缺失/网络/认证分流引导）、连接编辑回填远程字段、MILESTONES/CHANGELOG/用户文档同步 | 发布说明 + 已知问题清单；M1/M2 用户可平滑升级 |

估点单位：1 点 ≈ 一个 agent 半天；含测试与文档。

## 7. 风险表

| 风险 | 等级 | 缓解 |
|---|---|---|
| arm64 交叉编译 | 低 | 方案 A 为纯 JS 驱动，无本地原生依赖；Rust 侧零新增依赖。若未来改方案 B 则升为高，保持 A |
| Windows 服务发现/连通性 | 中 | 内网 host、非默认端口、SSL（PG `ssl*`/`mysql2 ssl` 经 `extra` 透传）；connect 错误消息保留驱动原文（脱敏后）；M3 加错误分类引导与"测试连接"按钮 |
| 凭证处理 | 中 | 不落盘明文（`db.json` 无密码字段，已有测试）；错误消息 `sanitize` 脱敏（已有测试）；`keyring://` 未实现需显式报错（已有）；后续接 SecretStore lane 统一处理 |
| sidecar Node 缺失降级 | 中 | list/save/delete 可用、connect 报 `DbUnavailable`（已有单测）；UI 在 connect 失败时引导 runtime 设置页（复用 runtime lane 的 NeedManual 提示） |
| 大结果集 OOM/IPC 膨胀 | 中 | M2 引入 `MAX_CELL_BYTES`/`MAX_RESULT_BYTES` 字节预算（3.5），对齐 `MAX_STREAM_BYTES` 先例 |
| 方言词法歧义（MySQL `#` 注释、`/*! */` 可执行注释） | 低 | 已 fail-closed（`sql.rs:324-338`），有多组 payload 测试 |
