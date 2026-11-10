# 插件市场实施设计

> 状态：设计定稿，待执行（插件系统 v0 收官基线，docs/EXTENSIONS.md）。
> 范围：市场 v1——静态 index + 多源镜像下的搜索 / 安装 / 更新 / 卸载；签名两档；桌面端先行。
> 前提：v0 已完成本地目录安装与命令面（EXTENSIONS.md §3），明确未实现市场 / 签名 / 自动更新（§7、§8）。

## 0. 结论（先行）

- **推荐方案：C（静态 index + 多源镜像）**。registry 只是一个带 sha256 的 JSON 索引文件，可从任意 https / file URL 或镜像加载，插件包走同一机制——与发布线的 updater-manifest（`docs/UPDATER.md`、`scripts/gen-update-manifest.mjs`）完全同构：零服务端运维、零新增重型依赖（`lc-core` 已有 `ureq`/`zip`/`sha2`，`crates/lc-core/Cargo.toml:18-29`）、天然支持国内镜像回退。
- **最大风险**：
  1. **国内网络**：GitHub 裸连不稳，默认源与包下载都可能失败；
  2. **供应链**：强制签名档（M3）落地前，HTTPS+sha256 只防传输篡改，不防源被攻陷或恶意上架；而 Node 插件是可信代码（§7），curated 审查是社会工程唯一主防线；
  3. **一致性**：index 与包内清单漂移、多源合并冲突、preview 版本比较语义错误，会导致装错版本或半安装状态。

## 1. 现状基线

| 层 | 位置 | 可复用度 |
|---|---|---|
| 安装态布局 | `ExtPaths`（`crates/lc-core/src/ext/mod.rs:247-263`）：`~/.lite-code/extensions/<id>/plugin.json` | 市场安装直接落位同一目录，装完即被既有发现链接管 |
| 启用状态 | `~/.lite-code/extensions.json` `{disabled:[...]}`，原子写（`crates/lc-core/src/ext/state.rs:80-99` 用 `config::write_atomic`） | 卸载需从 disabled 清理；更新保留 disabled 状态 |
| 命令面 | 8 个 `ext_*` Tauri 命令，薄转发、业务在 `lc-core`（`apps/desktop/src-tauri/src/ext.rs:77-120`） | 新命令照抄此模式，custom DTO，不动 `lc-proto` 契约 |
| 清单校验 | §1 全部规则（id/version/runtime/main 越界/activationEvents/contributes 各栏/依赖环）在 `crates/lc-core/src/ext/manifest.rs` | index 的 manifest 副本与包内 plugin.json 双重比对后，仍走同一条校验链 |
| HTTP 拉取先例 | `apps/desktop/src-tauri/src/updater.rs:183-209`：ureq、10s 超时、响应上限、仅 https+回环、env 覆盖 URL | index/包下载直接复用该套约束 |
| 静态清单生成 | `scripts/gen-update-manifest.mjs`：sha256 必填、不伪造签名、semver 校验 | 新脚本 `scripts/gen-plugin-index.mjs` 同风格 |
| semver 比较 | 手写 `Version/PreId/Ord`（`updater.rs:380-450`，含 prerelease 数字段<字母段） | **移到 `lc-core` 共享**，市场与更新器同一语义 |
| 安全边界 | §7：插件代码不进渲染进程、Node 插件可信、WASM 强隔离、`scrub_secret_env` | 市场不削弱任何一条，只新增 UI 提示义务（§3.8） |
| 解压/哈希 | `zip 9.3`（deflate-only）、`sha2` 已在 `lc-core` | 零新增依赖 |
| 鸿蒙 | `crates/lc-ohos` 未转发 ext 命令（grep 无 `ext_*` 接线；§3 亦标注 shim 未转发 `ext_read_grammar`） | 市场桌面先行；逻辑在 `lc-core`，ohos 后续可接 |

## 2. 方案对比

### A. GitHub 仓库作 registry（index 挂 releases 资产）

- 优点：零基建——复用发布线（`updater-manifest` 滚动 release 模式），PR 即审核入口，GitHub 提供 HTTPS 与历史。
- 缺点：国内连通性不可控（正是 updater 默认源的既有痛点）；无独立审核语义（任何人给仓库提 PR 即可触达用户）；单一厂商锁定。
- **定位**：不作为独立方案，被吸收为方案 C 的**默认承载形态**（官方源 = marketplace 仓库的 release 资产）。

### B. 独立 registry 服务（自建 HTTP API + 数据库）

- 优点：可控——上传管道、审核状态机、下载统计、按 id 精确查询都可定制。
- 缺点：运维成本与可用性责任全在团队（国内部署 + 备案 + 带宽）；服务端一旦失守可同时篡改 index 与包（sha256 同在库内）；与「不新增重型依赖、轻发布线」的仓库气质相悖。
- 否决。未来若插件规模上三位数再评估，index 线格式无需变（C 可平滑升级到 B 的 API 后端）。

### C. 静态 index + 多源镜像（推荐）

- registry 只是一个 JSON 索引文件：客户端从**任意** URL（GitHub release 资产 / git raw / 国内对象存储镜像 / 本地 `file://`）加载；插件包由 index 中的 `dist.url` 指向，走**同一套**多源与校验机制。VS Code 市场本质即「index + 包」，此处把 index 静态化。
- 优点：与 updater-manifest 同构（团队已验证的发布链路与客户端解析代码可直接借鉴）；多镜像天然解决国内可用性；`file://` 源让离线内网与测试零成本；无服务端，攻击面最小。
- 缺点：搜索只能是「拉全量 index + 本地过滤」（索引量增大后需分段，见 §3.1 上限）；多源合并语义需要仔细定（§3.3）；index 与包的一致性要靠校验而非事务（§3.5、风险表）。
- **结论：采用 C**，A 作为其默认官方源形态，B 明确否决并记录升级路径。

## 3. 推荐方案详细设计

### 3.1 index 格式

文件名建议 `index.json`，UTF-8，上限 **4 MiB**（单源条目上限 4096；超限时客户端报 `contract_mismatch` 并提示换源/升级客户端，防恶意巨型响应撑爆内存——上限模式同 updater 的 `MAX_MANIFEST_BYTES`）：

```json
{
  "version": 1,
  "generated_at": "2026-11-11T00:00:00Z",
  "plugins": [
    {
      "id": "example.hello",
      "version": "0.1.0",
      "manifest": { "id": "example.hello", "version": "0.1.0", "runtime": "node" },
      "dist": { "url": "https://mirror.example/example.hello-0.1.0.zip", "sha256": "<64 hex>" },
      "signature": "",
      "engines": { "lite-code": ">=0.6.1" },
      "deps": ["example.dep"]
    }
  ]
}
```

字段规则：

- `version`：index 线格式版本。客户端只接受 `1`，否则 `contract_mismatch`（fail-closed，同 `SqlDialect` 闸门风格）。
- `plugins[].id`：复用 §1 形态（小写字母/数字/点/线）；同 index 内重复 id → 整份 index 判无效（防混淆攻击）。
- `version`：semver 字符串；解析失败 → 该条目跳过并计数，不毒化整份 index。
- `manifest`：**§1 plugin.json 的逐字内嵌副本**。它与条目的 `id`/`version` 必须一致（客户端与生成端双向校验）。`manifest` 的存在让 `ext_search` 无需下载即可展示描述/权限/贡献点；**安装态的唯一权威清单仍是磁盘上的 plugin.json**，安装后由既有 `list_plugins` 校验链（§1 全部规则，含依赖环、contributes 各栏）接管。
- `dist.url`：https 或回环 http（同 updater 的 `is_https_or_loopback`）；`file://` 允许但条目带 `fileSource: true` 标记，UI 展示来源时区分。`dist.sha256`：64 位十六进制，必填。
- `signature`：minisign 签名（trusted comment + base64），空串=未签名（模式同 updater 不伪造签名）。
- `engines` / `deps`：从 manifest 提取的冗余字段，用于安装前 fail-closed 预检（engines 不满足拒绝安装）与依赖解析；与 manifest 不一致时以 manifest 为准并计警告。

### 3.2 插件包格式

- `.zip`（仅 stored/deflate，与 `lc-core` 的 zip features 一致；拒绝加密条目与其他压缩法）。
- 包内**单一顶层目录** `<id>/`，`plugin.json` 位于 `<id>/plugin.json`——包即「安装目录的 zip」，解压后按 index id 落位 `<user_dir>/<id>/`。
- 上限：包体 ≤ 64 MiB；解压后单插件 ≤ 256 MiB；条目数 ≤ 4096。
- 解压防御（zip-slip）：每个条目路径规范化后必须落在目标 tmp 目录内——拒绝绝对路径、`..`、盘符前缀（`C:\` / `/`）；条目名与 `plugin.json` 所在层之外的层数不设限但计入条目数上限。

### 3.3 源管理与多镜像

- 源清单：`~/.lite-code/plugin-registries.json`：

```json
{ "sources": [
  { "name": "official", "url": "https://...", "require_signatures": false, "pubkey": "" }
] }
```

- 内置默认官方源：常量放在桌面层（同 updater 的 `DEFAULT_MANIFEST_URL`，`updater.rs:17-18`），可用 env `LC_PLUGIN_REGISTRY_URL` 覆盖（测试缝，同 `LC_UPDATE_MANIFEST_URL`）；**builtin 源不可删除、可禁用**。
- URL 校验：`ext_registry_add` 只接受 https / 回环 http / `file://`（file 源在 UI 标注「本地源，自行承担信任」）。
- 多源语义：`ext_search` 并行拉取全部启用源的 index，内存缓存 TTL 5 分钟（`refresh=true` 强制）；**按 id 去重，取 semver 最高版本**；某源失败不毒化结果——该源条目缺失，源名记入返回的 `staleSources` 供 UI 提示「源 X 暂时不可用」。
- 多镜像（M3 完整落地，M1 起语义就位）：同一 index 可托管多份镜像 URL，回退顺序=源列表顺序；per-source 超时 10s、整体搜索预算 30s。

### 3.4 命令面（Tauri custom DTO，不动 lc-proto）

| 命令 | 签名 | 说明 |
|---|---|---|
| `ext_registry_list` | `() -> RegistryList` | `{sources:[{name,url,builtin,requireSignatures,hasPubkey}]}` |
| `ext_registry_add` | `(name, url, requireSignatures?, pubkey?) -> RegistryList` | URL 白名单校验；pubkey 仅本地配置，禁止网络获取（无 TOFU） |
| `ext_registry_remove` | `(name) -> RegistryList` | builtin 拒绝 |
| `ext_search` | `(query?, refresh?) -> MarketSearchResult` | `results:[MarketPlugin]` + `staleSources:[name]`；MarketPlugin 含 id/version/description/runtime/engines/deps/permissions/source/signatureState(signed\|unsigned\|enforced)/installedVersion/updateAvailable |
| `ext_install` | `(id, version?, source?, force?) -> PluginList` | 返回 `PluginList` 复用既有 UI 刷新链路（同 `ext_reload` 返回约定） |
| `ext_update` | `(id?, force?) -> PluginList` | id 空=全部可更新项；保留 disabled 状态 |
| `ext_uninstall` | `(id) -> PluginList` | 见 §3.5 |

- 错误码复用既有 `ErrorCode`（`invalid_argument`/`contract_mismatch`/`not_found`/`internal`），**不新增契约码**、无契约 bump；DTO 全部 camelCase（同 `PluginList` 先例）。
- 鸿蒙：`lc-ohos` 暂不接线（现状 ext 命令亦未转发）；逻辑全在 `lc-core`，后续接入只需在 shim 补转发。

### 3.5 安装 / 更新 / 卸载流程

**install(id, version?, source?, force?)**：

1. 条目定位：version 缺省取该 id 在所有源的 semver 最高版本；source 缺省=命中源的列表序第一个。
2. 预检：engines 不满足 → 报错（`force=true` 跳过，UI 确认对话框承担 force）；deps 在本地与所有源都缺失 → M1 报错并列出缺失 id（M2 起自动装，见下）。
3. 下载到 `<user_dir>/.tmp/<id>-<version>-<rand>.zip`（同分区，保证 rename 原子；上限 64 MiB）。
4. 校验：sha256 →（M3）签名档校验 → 解压到 `<user_dir>/.tmp/<id>/`（防穿越，§3.2）→ **包内 plugin.json 与 index 条目比对 id/version，不一致即 `contract_mismatch` 拒绝**（index-包一致性绑定）。
5. 落位：rename 到 `<user_dir>/<id>/`。Windows rename 不覆盖 → 先删旧目录（先例：`sidecar.rs` install 的「临时文件 + rename，Windows 下先删旧文件」）。
6. 刷新：走 `list_plugins_with_node` 返回 `PluginList`——§1 全部校验自然生效，坏包只会出现 `error` 字段而不会破坏宿主。

**update(id?)**：对每个已安装插件取 index 最高版本，`semver 严格大于` 才更新（等于/更低=no-op）；更新即走 install 的交换路径，**不动 disabled 集合**。version 比较用从 `updater.rs` 移入 `crates/lc-core/src/version.rs` 的共享实现（prerelease 数字段<字母段、release>prerelease；`0.6.1-preview.2 < 0.6.1-preview.10` 按数值比较），格式校验继续由生成端 `gen-update-manifest.mjs` 的 semver 正则承担，两端互补。

**uninstall(id)**：

1. 路径白名单：规范化后必须等于 `<user_dir>/<id>`，拒绝 project 级插件与一切穿越（project 级仍只能手动管理）。
2. 先改名 `<id>.uninstall-bak`（失败可回滚），再从 `extensions.json` 的 disabled 移除，确认无误后删 bak。
3. 刷新返回 `PluginList`。

**原子性与半安装恢复**：tmp+rename 保证任何中断点都只剩 `.tmp/` 或 `*.uninstall-bak` 垃圾；`lc-core::ext` 初始化时 GC 这两类残留（sidecar 启动即用的惰性模式，不阻塞启动）。下载/校验/解压任一失败：tmp 全删，最终目录零变更。

**依赖（M2）**：install 时 deps 缺失 → 同源优先、任意源兜底自动安装，深度上限 8、环检测（复用 §1 依赖环报错语义）；链中失败已装依赖保留（每个依赖本身是合法插件），主插件不落位。

### 3.6 磁盘配额

- 三级上限：单包下载 64 MiB、单插件解压 256 MiB、`extensions` 目录总量默认 **1 GiB**（env `LC_EXT_QUOTA_MB` 覆盖）。
- 超限：拒绝安装/更新并提示卸载路径；配额在落位前核算（tmp 阶段统计解压总量）。
- GC 顺带清理超龄 tmp（>24h）。

### 3.7 安全模型

**两档信任（每源可配）**：

- **警告档（M1 默认）**：HTTPS + sha256 完整性；`signature` 缺失允许安装，但 `signatureState=unsigned` 必须进确认 UI 与结果徽标。
- **强制档（M3，`require_signatures=true`）**：源配置 pinned minisign 公钥（base64，只来自本地 `plugin-registries.json`）；包必须有签名且验证通过（trusted comment 匹配），否则拒绝安装。密钥与 updater 签名（`TAURI_SIGNING_*`，minisign 同族）**分钥**——应用更新链与插件供应链爆炸半径隔离。
- 依赖：M3 引入 `minisign-verify`（纯 Rust、体积小；**必须先过 `deny.toml` 许可白名单**；若被拒，备选 `ed25519-dalek` 自解析 trusted comment）。不引入 `ring`/`aws-lc-sys` 系（arm64 交叉编译高危，db-mysql-pg 方案对比 B 的教训）。

**既有防线（复述 §7，市场不得削弱）**：插件代码永不进渲染进程；Node 插件=可信代码，`permissions` 仅提示；插件工具输出包 `[UNTRUSTED PLUGIN CONTENT]` 边界且一律 `Risk::Mutating` 走既有审批；宿主/WASM 子进程 `scrub_secret_env`；WASM 才是强隔离层。

**市场新增的提示义务（v1 的 UI 硬要求）**：

- 安装确认对话框必显：id、version、runtime、**来源源名**、permissions 列表、`signatureState`、包大小；`runtime=node` 时固定文案明示「该插件将以你的用户权限执行任意代码」（§7 的可信代码语义直达用户）。
- 浏览列表对 unsigned 与 `file://` 源条目打可见徽标。

**审查管道取舍**：v1 = **curated 单官方源**，PR 制提交（marketplace 仓库内 `plugins/<id>/<version>/`，CI 校验 manifest、计算 sha256、可选签名后重生成 index——`scripts/gen-plugin-index.mjs` 与 `gen-update-manifest.mjs` 同风格：无第三方依赖、不伪造签名）；**不做开放上传**。第三方源可任意添加但 UI 标「未审查」。理由：Node 插件可信代码的本质决定审查是最后一道闸，与 VS Code 市场（curated + 签名身份）对齐；开放上传在签名强制档稳定运行前不考虑。

### 3.8 UI 落点

- **ExtensionsPanel 加「浏览」标签**（`apps/web/src/components/ExtensionsPanel.vue`，现 849 行为单列表）：顶部两个 tab——「已安装」（现状原样）/「浏览市场」（搜索框 + 结果卡片 + 徽标 + 安装/更新按钮 + §3.7 确认对话框）。命令面板**不加**市场命令的完整交互，只留跳转入口——浏览是富交互场景，面板是正确落点。
- `ext_open_dir` 保持现状：市场只管理它在 `<user_dir>` 内安装的目录；手动放插件的老路径不变，两个入口在「已安装」列表汇合。

### 3.9 代码落点

- `crates/lc-core/src/ext/marketplace/`（新）：`index.rs`（解析/校验）、`source.rs`（源清单读写 + 拉取 + 缓存 + 回退）、`package.rs`（zip 校验/解压/防穿越）、`install.rs`（下载/sha256/签名/交换/GC/配额）。
- `crates/lc-core/src/version.rs`（新）：接收从 `apps/desktop/src-tauri/src/updater.rs` 移入的 `Version/PreId/Ord`；updater 改为引用，语义单一来源。
- `apps/desktop/src-tauri/src/ext.rs`：新增 6 个命令的薄转发；`apps/desktop/src-tauri/src/lib.rs:820-824` 区域注册。
- `apps/web/src/services/marketplace.ts`（invoke 封装，同 `services/extensions.ts` 风格）+ `ExtensionsPanel.vue` tab + `locales/zh-CN.json`/`en.json`。
- `scripts/gen-plugin-index.mjs` + 配套单测（仿 `gen-update-manifest.test.mjs`）。

## 4. 测试策略

- **index 解析**（单测）：合法/缺字段/重复 id/非法 semver/sha256 非 64hex/manifest 与条目 id-version 漂移/超大响应（>4 MiB）/条目数超限/线格式 version≠1。
- **版本比较**：迁移 updater 的 semver 测试矩阵；新增 prerelease 段用例（`preview.2 < preview.10`、`1.0.0-alpha < 1.0.0`、release>prerelease）。
- **fake server**：`std::net::TcpListener` 写 <100 行最小 HTTP 响应器（零新依赖），覆盖 200/404/超时/截断连接/Content-Length 撒谎；`file://` 与回环 http 源端到端各一。
- **下载校验**：sha256 不匹配拒绝且不落位；M3 签名通过/失败/强制档缺签名三态。
- **原子安装**：测试内用 `zip` crate 生成正样本；手工字节构造坏 zip、`../evil`、`/abs`、`C:\` 条目断言拒绝；预置 `.tmp` 垃圾与 `*.uninstall-bak` 断言启动 GC 清理；Windows rename 覆盖路径在 tmpdir 内模拟。
- **配额**：三档上限各一拒绝用例。
- **网络降级**：源 1 挂 → 回退源 2 且结果合并；`staleSources` 上报正确。
- **web 侧**：`marketplace.ts` 的 spec 单测（mock invoke，仿 `extensions.spec.ts`）+ 确认对话框渲染 permissions/unsigned 徽标的组件测试。
- 无真实外网依赖：全部网络用例走 loopback/file，CI 离线可跑（同 `gen-update-manifest` 约束）。

## 5. 里程碑切片（每片独立可发布）

| 切片 | 内容 | 估点 | 独立发布条件 |
|---|---|---|---|
| **M1 index+安装** | index 线格式+解析；`version.rs` 移入 lc-core；`plugin-registries.json`+3 个 registry 命令；`ext_search`；`ext_install`（下载/sha256/一致性/防穿越/tmp+rename/GC/配额）；ExtensionsPanel 浏览 tab+确认 UI；`gen-plugin-index.mjs` | 6 | file:// 与 https（loopback 集成测试）源可搜索并安装，中断恢复无残留；已安装页零回归 |
| **M2 更新与卸载** | `ext_update`（单个/全部、semver 严格大于、disabled 保留）；`ext_uninstall`（白名单+disabled 清理）；deps 自动安装（深度/环护栏）；更新角标 | 3 | 装旧版→可更新→状态保留；卸载后目录与 `extensions.json` 干净 |
| **M3 签名与多镜像** | `minisign-verify` 接入（过 deny.toml）；per-source `require_signatures`+pubkey；强制档拒绝路径与徽标；多源回退排序；官方 marketplace 仓库+CI+镜像发布文档 | 4 | 未签名包在强制源被拒；多源 fail-over 集成测试过；发布链文档化 |

估点单位：1 点 ≈ 一个 agent 半天；含测试与文档。

## 6. 风险表

| 风险 | 等级 | 缓解 |
|---|---|---|
| 国内网络：GitHub 裸连不稳，默认源/包下载失败 | **高** | 多源镜像+回退（§3.3）；默认源同时发布到滚动 release 资产与国内可达镜像（对象存储/Gitee），env 可切；per-source 10s 超时+30s 总预算；失败源不毒化结果 |
| 供应链：index/包被篡改或源被攻陷 | **高**（M3 前） | sha256 绑定+HTTPS（M1）；签名强制档+分钥+本地 pinned pubkey（M3）；curated PR 制审查；包内 plugin.json 与 index 双向一致性校验 |
| index 与包一致性漂移（id/version 改了一个） | 中 | 安装时强制比对，不一致 `contract_mismatch`；生成端 CI 同规则校验；整源重复 id 判无效 |
| 恶意插件社会工程（仿冒知名 id、permissions 误导） | 中 | §3.7 确认 UI 硬要求（来源/权限/可信代码文案）；curated 列表+签名身份；开放上传不做；插件工具输出 UNTRUSTED 边界与审批已兜底执行面 |
| 存储配额失控（大包/多插件/tmp 残留） | 中 | 三级上限+`LC_EXT_QUOTA_MB`+落位前核算+GC（§3.6） |
| 半安装/降级损坏（中断、rename 失败） | 低 | tmp+rename+`.uninstall-bak` 可回滚+启动 GC（§3.5）；`write_atomic` 与 sidecar install 同构先例 |
| preview 版本比较语义分歧（更新器 vs 市场） | 低 | `Version/PreId/Ord` 移入 lc-core 单一来源，双方共用测试矩阵（§3.5） |

## 7. 假设（已自行裁定，待主 agent 确认）

- 官方 marketplace 仓库（如 `OpenALC/lite-code-marketplace`）尚未创建：M1 开发用 `file://`+loopback 假服务器；默认官方源 URL 为占位常量，发布前落定（同 updater 常量模式）。
- 包格式「单一顶层目录 `<id>/`」为新约定，无历史包兼容负担（v0 无市场）。
- `minisign-verify` 许可能过 `deny.toml`；不过则退回 `ed25519-dalek` 备选（§3.7）。
- 市场命令仅桌面 Tauri 层落地；`lc-ohos` 接入不在本计划范围。
- 默认配额 1 GiB 为经验值，随 M1 实测可调。
