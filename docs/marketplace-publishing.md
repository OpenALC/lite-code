# 插件市场发布端操作指南

> 面向维护者：在独立仓库（建议 `OpenALC/lite-code-marketplace`）搭建 curated
> 官方源。客户端（浏览/安装/更新/签名校验/多源镜像）已在应用内实现，见
> `docs/plans/plugin-marketplace.md` §3.7；本文只覆盖**发布端**。
>
> 状态：生成器 `scripts/gen-plugin-index.mjs` 已入库（含 5 项测试，CI 门禁）；
> 建仓后把本文与生成器复制过去即可开工。

## 1. 仓库布局

```
lite-code-marketplace/
├── plugins/
│   └── <id>/
│       └── <version>/            # 一个版本一个目录，只增不改（重发 = 新版本号）
│           ├── plugin.json       # 插件清单（逐字内嵌进 index）
│           ├── package.zip       # 安装包（客户端按 sha256 校验后原子安装）
│           └── package.zip.sig   # 可选：minisign 签名（官方源强制档要求）
├── index.json                    # CI 生成，勿手改
└── .github/workflows/index.yml   # 校验 + 重生成 index（见 §4）
```

约束（生成器与客户端双端校验）：

- 目录 `<id>` 必须等于 `plugin.json.id`，且是合法插件 id（非空、≤128、
  无空白/控制符/斜杠）；
- 目录 `<version>` 必须等于 `plugin.json.version`，且是合法 semver
  （`-preview.N` 等预发布段可用）；
- 同一 id 可并存多个版本；客户端按 semver 取最高。

## 2. 提交流程（PR 制）

1. 作者把打包好的插件目录放进 `plugins/<id>/<version>/` 提 PR；
2. 维护者审查（Node 插件=可信代码，审查是最后一道闸；清单 `permissions`
   只是提示，**不能**当作沙箱）；
3. 合并后 CI 重生成 `index.json` 并提交/发布。

## 3. 本地生成与验证

```bash
# 生成 index（写 stdout 或 --output）
node scripts/gen-plugin-index.mjs \
  --plugins plugins \
  --base-url https://raw.githubusercontent.com/OpenALC/lite-code-marketplace/main/plugins \
  [--require-signatures] \
  --output index.json
```

- `--base-url` 必须是 `https://`、`file://` 或回环 `http://127.0.0.1|localhost`
  （客户端 URL 白名单同规则）；`dist.url` 按
  `<base-url>/<id>/<version>/package.zip` 生成；
- 没有 `.sig` 时 `signature` 省略（客户端按未签名条目处理，UI 打标）；
  `--require-signatures` 收紧为缺签名即失败（官方源强制档）；
- 生成器**不伪造签名**：签名必须由签名者用私钥产出（见下）。

## 4. 签名（官方源强制档）

与市场设计一致的语义（`docs/plans/plugin-marketplace.md` §3.6）：

```bash
# 对包 sha256 的 64 位小写十六进制文本做 minisign 直签（legacy "Ed"），
# trusted comment 填入同一 hex —— 客户端做三重绑定校验。
sha=$(sha256sum plugins/<id>/<version>/package.zip | cut -d' ' -f1)
printf '%s' "$sha" | minisign -S -s <secret.key> -t "$sha" -x plugins/<id>/<version>/package.zip.sig
```

签名私钥由维护者保管（建议离线/受控 CI secret），与更新器签名分钥（爆炸
半径隔离）。

## 5. 仓库 CI（建议）

```yaml
# .github/workflows/index.yml（市场仓库）
name: index
on:
  push:
    branches: [main]
jobs:
  build-index:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - name: 校验并生成 index
        run: |
          node gen-plugin-index.mjs \
            --plugins plugins \
            --base-url https://raw.githubusercontent.com/${{ github.repository }}/main/plugins \
            --require-signatures \
            --output index.json
      - name: 提交 index
        run: |
          git config user.name "marketplace-bot"
          git config user.email "bot@users.noreply.github.com"
          git add index.json
          git diff --cached --quiet || git commit -m "chore: regenerate index"
          git push
```

把本仓 `scripts/gen-plugin-index.mjs` 与测试文件复制到市场仓库根目录即可
（无第三方依赖，Node 22 直跑）。

## 6. 客户端接入

应用内：设置 → 插件 → 「添加源」填 index 的 https URL；或环境变量
`LC_PLUGIN_REGISTRY_URL`（测试缝）。官方源可在发布前直接写进默认配置。
第三方源可任意添加，UI 固定标「未审查」。

## 7. 验收清单

- [ ] 生成器测试全绿：`node --test scripts/gen-plugin-index.test.mjs`（本仓 CI 已挂）
- [ ] 市场仓库 CI 每次 push 重生成 index 且 `--require-signatures` 通过
- [ ] 应用内从官方源搜索→安装→更新→卸载闭环，`signatureState` 显示 enforced
- [ ] 镜像场景：index 与包下载均有回退（客户端已实现，发布端只需保证 URL 可达）
