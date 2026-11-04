#Requires -Version 7.0
<#
.SYNOPSIS
  scripts/perf 纯逻辑自测（不依赖 GUI、不启动被测应用）。

.DESCRIPTION
  用临时 fixture 真实调用 compare.ps1（独立 pwsh 子进程，检查退出码与输出），
  覆盖 perf.yml compare step 的四种约定情况，外加两个边界与一个 comment.ps1 冒烟：

  1. 无回归          -> exit 0
  2. 回归 > 15%      -> exit 1
  3. null 指标       -> 跳过不误伤，exit 0 且输出含 SKIP 警告
  4. 多余未知指标    -> 容错 warn，exit 0
  5. 边界：回归恰好 15%（不"大于"）-> exit 0
  6. baseline 为 0 而 current > 0（residualProcs 新增）-> exit 1
  7. comment.ps1 冒烟：无 GH_TOKEN 时生成 markdown 并 exit 0

  本地运行：pwsh -NoProfile -File scripts/perf/selftest.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$perfDir = $PSScriptRoot
$comparePath = Join-Path $perfDir 'compare.ps1'
$commentPath = Join-Path $perfDir 'comment.ps1'
foreach ($f in @($comparePath, $commentPath)) {
  if (-not (Test-Path $f)) { Write-Error "缺少被测脚本 $f"; exit 2 }
}

$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("perf-selftest-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

$script:passed = 0
$script:failedCases = [System.Collections.Generic.List[string]]::new()

function New-FixtureFile {
  param([string]$Name, [hashtable]$Map)
  # 固定五字段 + Map 中额外键（用于未知指标用例）；null 值保持 null
  $o = [ordered]@{
    trayRssMb     = if ($Map.Contains('trayRssMb')) { $Map['trayRssMb'] } else { $null }
    activeRssMb   = if ($Map.Contains('activeRssMb')) { $Map['activeRssMb'] } else { $null }
    coldStartMs   = if ($Map.Contains('coldStartMs')) { $Map['coldStartMs'] } else { $null }
    warmStartMs   = if ($Map.Contains('warmStartMs')) { $Map['warmStartMs'] } else { $null }
    residualProcs = if ($Map.Contains('residualProcs')) { $Map['residualProcs'] } else { $null }
  }
  foreach ($k in $Map.Keys) { if (-not $o.Contains($k)) { $o[$k] = $Map[$k] } }
  $path = Join-Path $tmpDir $Name
  $o | ConvertTo-Json | Set-Content -Path $path -Encoding utf8NoBOM
  return $path
}

function Invoke-CompareFixture {
  param([string]$BaselineFile, [string]$CurrentFile, [double]$MaxPct = 15)
  # 独立子进程执行，exit code 语义与 CI 完全一致
  $out = & pwsh -NoProfile -File $comparePath `
    -Baseline $BaselineFile -Current $CurrentFile -MaxRegressionPct $MaxPct *>&1
  $text = ($out | ForEach-Object { "$_" }) -join "`n"
  return @{ ExitCode = $LASTEXITCODE; Output = $text }
}

function Assert-Case {
  param([string]$Name, [bool]$Condition, [string]$Detail)
  if ($Condition) {
    $script:passed++
    Write-Host ("  PASS  {0}" -f $Name)
  } else {
    $script:failedCases.Add($Name)
    Write-Warning ("  FAIL  {0}  {1}" -f $Name, $Detail)
  }
}

try {
  Write-Host "自测目录: $tmpDir"

  # ---- 用例 1：无回归 -> exit 0 -------------------------------------------
  Write-Host '[1] 无回归（数值相同）期望 exit 0'
  $b1 = New-FixtureFile -Name 'b1.json' -Map @{ coldStartMs = 1000; residualProcs = 0 }
  $c1 = New-FixtureFile -Name 'c1.json' -Map @{ coldStartMs = 1000; residualProcs = 0 }
  $r1 = Invoke-CompareFixture -BaselineFile $b1 -CurrentFile $c1
  Assert-Case '1-no-regression-exit0' ($r1.ExitCode -eq 0) "实际 exit=$($r1.ExitCode)`n$($r1.Output)"
  Assert-Case '1-no-regression-has-table' ($r1.Output -match 'coldStartMs') '输出中未见对比表'

  # ---- 用例 2：回归 > 15% -> exit 1 ----------------------------------------
  Write-Host '[2] 回归 20%（1000 -> 1200）期望 exit 1'
  $b2 = New-FixtureFile -Name 'b2.json' -Map @{ coldStartMs = 1000; residualProcs = 0 }
  $c2 = New-FixtureFile -Name 'c2.json' -Map @{ coldStartMs = 1200; residualProcs = 0 }
  $r2 = Invoke-CompareFixture -BaselineFile $b2 -CurrentFile $c2
  Assert-Case '2-regression-exit1' ($r2.ExitCode -eq 1) "实际 exit=$($r2.ExitCode)`n$($r2.Output)"
  Assert-Case '2-regression-marks-fail' ($r2.Output -match 'FAIL') '输出中未标 FAIL'

  # ---- 用例 3：null 指标跳过不误伤 -> exit 0 -------------------------------
  Write-Host '[3] null 指标（基线 warmStartMs=null、本次 coldStartMs=null）期望 exit 0 且 SKIP'
  $b3 = New-FixtureFile -Name 'b3.json' -Map @{ coldStartMs = 1000; warmStartMs = $null; residualProcs = 0 }
  $c3 = New-FixtureFile -Name 'c3.json' -Map @{ coldStartMs = $null; warmStartMs = $null; residualProcs = 0 }
  $r3 = Invoke-CompareFixture -BaselineFile $b3 -CurrentFile $c3
  Assert-Case '3-null-skip-exit0' ($r3.ExitCode -eq 0) "实际 exit=$($r3.ExitCode)`n$($r3.Output)"
  Assert-Case '3-null-skip-warns' ($r3.Output -match '跳过') '输出中未见 SKIP 警告'

  # ---- 用例 4：多余未知指标容错 -> exit 0 ----------------------------------
  Write-Host '[4] 本次含未知指标 editorOpenMs 期望 exit 0 且 warn'
  $b4 = New-FixtureFile -Name 'b4.json' -Map @{ coldStartMs = 1000; residualProcs = 0 }
  $c4 = New-FixtureFile -Name 'c4.json' -Map @{ coldStartMs = 1000; residualProcs = 0; editorOpenMs = 123 }
  $r4 = Invoke-CompareFixture -BaselineFile $b4 -CurrentFile $c4
  Assert-Case '4-unknown-metric-exit0' ($r4.ExitCode -eq 0) "实际 exit=$($r4.ExitCode)`n$($r4.Output)"
  Assert-Case '4-unknown-metric-warns' ($r4.Output -match '未知指标') '输出中未见未知指标警告'

  # ---- 用例 5：边界，回归恰好 15%（严格大于才判） -> exit 0 -----------------
  Write-Host '[5] 边界：1000 -> 1150（恰好 15%）期望 exit 0'
  $b5 = New-FixtureFile -Name 'b5.json' -Map @{ coldStartMs = 1000; residualProcs = 0 }
  $c5 = New-FixtureFile -Name 'c5.json' -Map @{ coldStartMs = 1150; residualProcs = 0 }
  $r5 = Invoke-CompareFixture -BaselineFile $b5 -CurrentFile $c5
  Assert-Case '5-boundary-15pct-exit0' ($r5.ExitCode -eq 0) "实际 exit=$($r5.ExitCode)`n$($r5.Output)"

  # ---- 用例 6：baseline 0、current > 0（residualProcs 新增）-> exit 1 -------
  Write-Host '[6] residualProcs 基线 0 -> 本次 1 期望 exit 1（除零保护：新增即回归）'
  $b6 = New-FixtureFile -Name 'b6.json' -Map @{ coldStartMs = 1000; residualProcs = 0 }
  $c6 = New-FixtureFile -Name 'c6.json' -Map @{ coldStartMs = 1000; residualProcs = 1 }
  $r6 = Invoke-CompareFixture -BaselineFile $b6 -CurrentFile $c6
  Assert-Case '6-zero-baseline-residual-exit1' ($r6.ExitCode -eq 1) "实际 exit=$($r6.ExitCode)`n$($r6.Output)"

  # ---- 用例 7：comment.ps1 冒烟（无 GH_TOKEN，仅打印） ----------------------
  Write-Host '[7] comment.ps1 冒烟：无 GH_TOKEN 期望 exit 0 且输出 markdown 表'
  $savedToken = $env:GH_TOKEN
  $env:GH_TOKEN = $null
  try {
    $m7 = New-FixtureFile -Name 'm7.json' -Map @{ coldStartMs = 1000; residualProcs = 0 }
    $out7 = & pwsh -NoProfile -File $commentPath -Metrics $m7 *>&1
    $text7 = ($out7 | ForEach-Object { "$_" }) -join "`n"
    Assert-Case '7-comment-no-token-exit0' ($LASTEXITCODE -eq 0) "实际 exit=$LASTEXITCODE`n$text7"
    Assert-Case '7-comment-has-table' ($text7 -match '性能摘要' -and $text7 -match 'n/a') '输出中未见摘要表或 n/a 占位'
  } finally {
    $env:GH_TOKEN = $savedToken
  }
} finally {
  Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
if ($script:failedCases.Count -gt 0) {
  Write-Host ("自测结果: FAIL — {0}/{1} 通过，失败用例: {2}" -f $script:passed, ($script:passed + $script:failedCases.Count), ($script:failedCases -join ', '))
  exit 1
}
Write-Host ("自测结果: PASS — {0}/{0} 用例通过" -f $script:passed)
exit 0
