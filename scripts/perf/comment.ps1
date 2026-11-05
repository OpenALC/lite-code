#Requires -Version 7.0
<#
.SYNOPSIS
  把 run.ps1 的四个指标渲染成 markdown 摘要；有 GH_TOKEN 时发 PR 评论，否则仅打印。

.DESCRIPTION
  调用契约（逐字对齐 .github/workflows/perf.yml comment step，勿改参数名）：
      ./scripts/perf/comment.ps1 -Metrics scripts/perf/out/metrics.json
  该 step 条件为 if: always() && pull_request，且 env 已注入 GH_TOKEN（github.token）。

  行为：
  - -Metrics 文件缺失或解析失败 → 按"全部 n/a"渲染并 warn，不抛错
    （measure 失败时评论仍应发出，报告"无数据"本身即有价值）。
  - $env:GH_TOKEN 非空且能确定 PR 号（GITHUB_EVENT_PATH 的 pull_request.number，
    回退 GITHUB_REF 的 refs/pull/<n>/merge）→ `gh pr comment <n> --body-file`。
  - gh 不存在 / 发布失败 / 无法确定 PR 号 → warn 并退回仅打印；评论失败不改变
    measure/compare 已产生的结论，故本脚本退出码不反映 gh 失败。

  指标与目标值来源：docs/PLAN.md §4、docs/CI.md §3。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Metrics
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

# 指标展示顺序、目标值（PLAN §4）
$Presentation = [ordered]@{
  trayRssMb     = '托盘常驻 RSS'
  activeRssMb   = '窗口活跃 RSS'
  coldStartMs   = '冷启动'
  warmStartMs   = '热启动'
  residualProcs = '残留进程'
}
$Targets = @{
  trayRssMb     = '< 60 MB'
  activeRssMb   = '< 400 MB'
  coldStartMs   = '< 3000 ms 首启 / < 1500 ms 热'
  warmStartMs   = '< 1500 ms'
  residualProcs = '0'
}
# 显示时的量纲后缀（数值原样透传，不做单位换算）
$Units = @{
  trayRssMb     = 'MB'
  activeRssMb   = 'MB'
  coldStartMs   = 'ms'
  warmStartMs   = 'ms'
  residualProcs = ''
}

function Format-Value([object]$Value, [string]$Unit) {
  if ($null -eq $Value) { return 'n/a（未接入/未测）' }
  if ($Unit) { return '{0} {1}' -f $Value, $Unit }
  return '{0}' -f $Value
}

$values = @{}
$dataNote = ''
if (Test-Path $Metrics) {
  try {
    $obj = Get-Content -Path $Metrics -Raw -Encoding UTF8 | ConvertFrom-Json -ErrorAction Stop
    foreach ($name in $Presentation.Keys) {
      $prop = $obj.PSObject.Properties[$name]
      $values[$name] = if ($null -ne $prop) { $prop.Value } else { $null }
    }
  } catch {
    Write-Warning "指标文件不是合法 JSON（$Metrics）：$_ ；按无数据处理"
    $dataNote = '（指标文件解析失败，以下为空值）'
  }
} else {
  Write-Warning "指标文件不存在（$Metrics）；measure 步骤可能失败，按无数据处理"
  $dataNote = '（指标文件缺失，measure 可能失败）'
}

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add('## Lite Code 性能摘要（perf）' + $dataNote)
$lines.Add('')
$lines.Add('| 指标 | 本次 | 目标 |')
$lines.Add('|---|---|---|')
foreach ($name in $Presentation.Keys) {
  $lines.Add(('| {0} | {1} | {2} |' -f $Presentation[$name], (Format-Value $values[$name] $Units[$name]), $Targets[$name]))
}
$lines.Add('')
$lines.Add('> trayRssMb 为"关闭主窗口后进程树 RSS"；当前 Lifecycle 关闭只隐藏、不销毁 WebView，与 PLAN §4 "< 60 MB（窗口已销毁）"口径不同。')
$lines.Add('> activeRssMb 依赖"3 文件 + 终端"的 UI 自动化入口，接入前为 n/a；coldStartMs 为"主窗口可见"近似值，非应用内可交互埋点。')
$lines.Add('> warmStartMs 为隐藏窗口后二次启动唤回主窗口的耗时；residualProcs 为 20 次窗口开关后的进程快照 diff，口径见 scripts/perf/run.ps1 头注释。')
$lines.Add('> 回归 > 15% 会在 compare 步骤判 FAIL（docs/PLAN.md §4）；基线更新需 `perf-baseline` 标签 + 人工合并。')

$markdown = ($lines -join "`n")

# ---- 发布 -----------------------------------------------------------------
$token = $env:GH_TOKEN
if (-not $token) {
  Write-Host 'GH_TOKEN 未设置，仅打印摘要：'
  Write-Host $markdown
  exit 0
}

$prNumber = $null
if ($env:GITHUB_EVENT_PATH -and (Test-Path $env:GITHUB_EVENT_PATH)) {
  try {
    $ev = Get-Content -Path $env:GITHUB_EVENT_PATH -Raw | ConvertFrom-Json
    $num = $ev.pull_request.number
    if ($null -eq $num) { $num = $ev.number }
    if ($null -ne $num) { $prNumber = [string]$num }
  } catch {
    Write-Warning "解析 GITHUB_EVENT_PATH 失败：$_"
  }
}
if (-not $prNumber -and $env:GITHUB_REF -match '^refs/pull/(\d+)/') {
  $prNumber = $Matches[1]
}
if (-not $prNumber) {
  Write-Warning '无法确定 PR 号（非 pull_request 上下文？），仅打印摘要：'
  Write-Host $markdown
  exit 0
}

$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) {
  Write-Warning 'GH_TOKEN 已设置但找不到 gh CLI，仅打印摘要：'
  Write-Host $markdown
  exit 0
}

$bodyFile = [System.IO.Path]::GetTempFileName()
try {
  Set-Content -Path $bodyFile -Value $markdown -Encoding utf8NoBOM
  $repoArgs = @()
  if ($env:GITHUB_REPOSITORY) { $repoArgs += @('--repo', $env:GITHUB_REPOSITORY) }
  & gh pr comment $prNumber @repoArgs --body-file $bodyFile
  if ($LASTEXITCODE -ne 0) {
    Write-Warning "gh pr comment 失败（exit $LASTEXITCODE）；摘要降级为本地打印："
    Write-Host $markdown
  } else {
    Write-Host "评论已发布到 PR #$prNumber"
  }
} finally {
  Remove-Item -Path $bodyFile -ErrorAction SilentlyContinue
}
exit 0
