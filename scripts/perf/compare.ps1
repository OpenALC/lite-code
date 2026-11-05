#Requires -Version 7.0
<#
.SYNOPSIS
  性能基线对比：逐指标回归检测。

.DESCRIPTION
  调用契约（逐字对齐 .github/workflows/perf.yml compare step，勿改参数名）：
      ./scripts/perf/compare.ps1 `
        -Baseline scripts/perf/baseline.json `
        -Current scripts/perf/out/metrics.json `
        -MaxRegressionPct $env:REGRESSION_Pct   # CI 中为 15

  判定规则（docs/PLAN.md §4、docs/CI.md §3）：
  - 四个指标均为"越大越差"，回归% = (current - baseline) / baseline * 100
  - 回归 > MaxRegressionPct（严格大于；恰好等于阈值不算回归）→ exit 1
  - 指标为 null / 缺失 → 跳过并 warn，不影响退出码（P1 阶段三项指标尚未接入）
  - baseline 为 0 且 current > 0 → 视为无穷大回归 → fail（residualProcs 场景）
  - 未知指标键 → warn 容错，不参与判定；以 "_" 开头的键视为注释/元数据，静默忽略
  - 文件缺失或 JSON 解析失败 → exit 2（配置错误，区别于回归的 exit 1）

  基线更新规则（docs/CI.md §3）：更新 scripts/perf/baseline.json 需要
  `perf-baseline` 标签 + 人工合并，不允许 PR 自动改基线——否则"改基线让 CI 绿"
  会成为默认操作。perf.yml 在本阶段不阻塞合并（P2 起阻塞）。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Baseline,

  [Parameter(Mandatory = $true)]
  [string]$Current,

  [ValidateRange(0, 1000)]
  [double]$MaxRegressionPct = 15
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

# 指标清单与单位（与 run.ps1 输出的五个字段一一对应）
$KnownMetrics = [ordered]@{
  trayRssMb     = 'MB'
  activeRssMb   = 'MB'
  coldStartMs   = 'ms'
  warmStartMs   = 'ms'
  residualProcs = '个'
}

function ConvertTo-MetricsObject([string]$Path, [string]$Label) {
  if (-not (Test-Path $Path)) {
    Write-Error "$Label 文件不存在: $Path"
    exit 2
  }
  try {
    return Get-Content -Path $Path -Raw -Encoding UTF8 | ConvertFrom-Json -ErrorAction Stop
  } catch {
    Write-Error "$Label 不是合法 JSON: $Path ($_)"
    exit 2
  }
}

$baselineObj = ConvertTo-MetricsObject -Path $Baseline -Label '基线'
$currentObj  = ConvertTo-MetricsObject -Path $Current  -Label '本次'

function Get-MetricValue($Obj, [string]$Name) {
  # 属性缺失返回 $null，与显式 null 同样按"跳过"处理
  $prop = $Obj.PSObject.Properties[$Name]
  if ($null -eq $prop) { return $null }
  return $prop.Value
}

$rows = @()
$failed = [System.Collections.Generic.List[string]]::new()
$skipped = [System.Collections.Generic.List[string]]::new()

foreach ($name in $KnownMetrics.Keys) {
  $unit = $KnownMetrics[$name]
  $b = Get-MetricValue -Obj $baselineObj -Name $name
  $c = Get-MetricValue -Obj $currentObj  -Name $name

  if ($null -eq $b -or $null -eq $c) {
    $skipped.Add($name)
    $missing = @()
    if ($null -eq $b) { $missing += '基线' }
    if ($null -eq $c) { $missing += '本次' }
    Write-Warning ("指标 {0} 在{1}中为 null/缺失，跳过对比（基线尚未收录该指标属预期，见 run.ps1 头注释）" -f $name, ($missing -join '与'))
    $rows += [pscustomobject]@{
      指标 = $name; 基线 = 'n/a'; 本次 = 'n/a'; 差值 = 'n/a'; 回归 = 'n/a'; 状态 = 'SKIP'
    }
    continue
  }

  if ($b -eq 0) {
    # 除零保护：基线为 0 时，本次 > 0 即无穷大回归
    if ($c -gt 0) {
      $failed.Add($name)
      $rows += [pscustomobject]@{
        指标 = $name; 基线 = $b; 本次 = $c; 差值 = "+$c"; 回归 = '∞'; 状态 = 'FAIL'
      }
      Write-Warning ("指标 {0}: 基线为 0、本次为 {1}（新增即回归，阈值 0），判 FAIL" -f $name, $c)
    } else {
      $rows += [pscustomobject]@{
        指标 = $name; 基线 = $b; 本次 = $c; 差值 = '0'; 回归 = '0%'; 状态 = 'OK'
      }
    }
    continue
  }

  $regressionPct = [math]::Round((($c - $b) / $b) * 100, 1)
  $delta = [math]::Round($c - $b, 1)
  $status = if ($regressionPct -gt $MaxRegressionPct) { 'FAIL' } else { 'OK' }
  if ($status -eq 'FAIL') { $failed.Add($name) }

  $rows += [pscustomobject]@{
    指标 = $name
    基线 = "$b $unit"
    本次 = "$c $unit"
    差值 = "{0}{1} {2}" -f ($(if ($delta -ge 0) { '+' } else { '' })), $delta, $unit
    回归 = "$regressionPct%"
    状态 = $status
  }
}

# 未知指标容错：多出的键 warn 但不判失败；"_" 开头视为注释/元数据静默忽略
foreach ($obj in @(@{ label = '基线'; o = $baselineObj }, @{ label = '本次'; o = $currentObj })) {
  foreach ($prop in $obj.o.PSObject.Properties) {
    if ($KnownMetrics.Contains($prop.Name) -or $prop.Name.StartsWith('_')) { continue }
    Write-Warning ("{0}文件含未知指标 '{1}'，忽略不参与判定" -f $obj.label, $prop.Name)
  }
}

Write-Host ''
Write-Host ("Lite Code 性能对比（阈值: 回归 > {0}% 判 FAIL）" -f $MaxRegressionPct)
$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

if ($failed.Count -gt 0) {
  Write-Host ("结果: FAIL — 超阈值指标: {0}" -f ($failed -join ', '))
  exit 1
}
Write-Host ("结果: PASS — 判定 {0} 项，跳过 {1} 项（{2}）" -f ($rows.Count - $skipped.Count), $skipped.Count, ($skipped -join ', '))
exit 0
