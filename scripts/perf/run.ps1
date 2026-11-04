#Requires -Version 7.0
<#
.SYNOPSIS
  Lite Code 性能测量 harness（P1，scripts/perf lane 独占）。

.DESCRIPTION
  调用契约（逐字对齐 .github/workflows/perf.yml measure job，勿改参数名）：
      ./scripts/perf/run.ps1 -Out scripts/perf/out/metrics.json
  指标定义与目标值：docs/PLAN.md §4、docs/CI.md §3。

  产出 JSON，字段名固定五个（compare.ps1 / comment.ps1 依赖这些名字）：
      trayRssMb, activeRssMb, coldStartMs, warmStartMs, residualProcs

  当前实现状态（P1 阶段，如实用近似）：
  - coldStartMs    真测，近似值。预热 1 次后连续启动 ColdStartRuns 次取中位数：
                   计时起点 = Start-Process 之前；终点 = 主窗口句柄首次非零
                   （即"进程稳定出现"）。真正的"冷启动到可交互"必须等应用内
                   埋点（启动耗时经 lc-proto 事件打到 stdout）后替换此近似。
  - residualProcs  真测，骨架。开关前进程快照 -> N 次"启动-优雅关闭"循环 ->
                   快照 diff，残留 = 出现在 after 且不在 before 的本应用进程数。
                   注意：当前只能做进程级开关；"开关窗口"要求应用常驻托盘、
                   仅销毁/重建窗口，等 P1 Lifecycle 托盘落地后把循环体替换为
                   窗口开关（Toggles 次数对齐 PLAN §4 的 20 次）。
  - trayRssMb      null。接入时机：P1 Lifecycle 实现托盘后——窗口销毁、应用
                   常驻托盘，静置 10s 采样 RSS（PLAN §4 "窗口销毁后 10s 采样"）。
  - activeRssMb    null。接入时机：P1 Lifecycle 提供窗口场景脚本入口后——打开
                   3 文件 + 终端场景，稳定后采样 RSS（目标 < 400 MB）。
  - warmStartMs    null。接入时机：托盘常驻（应用不退出）+ 应用内"可交互"
                   埋点两者齐备，才有可测的热启动；二次冷启动不算 warm start。
#>
[CmdletBinding()]
param(
  # 输出 JSON 路径；相对路径按当前工作目录解析（CI 传 scripts/perf/out/metrics.json）
  [Parameter(Mandatory = $true)]
  [string]$Out,

  # 被测可执行文件，默认仓库 target/release 产物（perf.yml 会先 cargo build --release）
  [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\release\lc-desktop.exe'),

  # 冷启动采样次数（取中位数，压平单次抖动；1 次预热不计时）
  [ValidateRange(1, 10)]
  [int]$ColdStartRuns = 3,

  # 残留进程检测的开关次数（对齐 docs/PLAN.md §4 "20 次开关"）
  [ValidateRange(1, 100)]
  [int]$Toggles = 20,

  # 单次启动等待主窗口出现的超时（超时按失败启动处理）
  [ValidateRange(1000, 60000)]
  [int]$StabilityTimeoutMs = 15000
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$AppName = 'lc-desktop'   # 快照匹配的进程名；WebView2 子进程归 Lifecycle 的 ProcessSupervisor 管，接入后可扩展

function Resolve-CiPath([string]$p) {
  # 相对路径按当前目录解析，绝对路径原样返回
  if ([System.IO.Path]::IsPathRooted($p)) { return $p }
  return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $p))
}

function Wait-MainWindowShown {
  param([System.Diagnostics.Process]$Proc, [int]$TimeoutMs)
  # 轮询主窗口句柄；句柄非零 = 窗口已创建并显示（"进程稳定出现"的近似判据）
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
    if ($Proc.HasExited) { return $false }
    try {
      $p = Get-Process -Id $Proc.Id -ErrorAction Stop
      if ($p.MainWindowHandle -ne 0) { return $true }
    } catch {
      return $false   # 进程消失
    }
    Start-Sleep -Milliseconds 50
  }
  return $false
}

function Stop-AppGracefully {
  param([System.Diagnostics.Process]$Proc)
  if ($Proc.HasExited) { return }
  # 先发 WM_CLOSE（对应用户关窗），5s 不退再强杀（强杀本身可能制造残留，会如实计入 diff）
  $null = $Proc.CloseMainWindow()
  if (-not $Proc.WaitForExit(5000)) {
    Write-Warning "进程 $($Proc.Id) 未响应 WM_CLOSE，强制结束"
    Stop-Process -Id $Proc.Id -Force -ErrorAction SilentlyContinue
    $null = $Proc.WaitForExit(3000)
  }
}

function Get-AppSnapshot {
  # 开关前后的进程快照：本应用进程的 Id 列表
  @(Get-Process -Name $AppName -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
}

# ---- 前置检查 -------------------------------------------------------------
$exePath = Resolve-CiPath $Exe
if (-not (Test-Path $exePath)) {
  Write-Error "未找到被测程序 $exePath；perf.yml 在本步之前会执行 cargo build --release，请先构建"
  exit 2
}
Write-Host "被测程序: $exePath"

# ---- coldStartMs（近似） --------------------------------------------------
# 预热 1 次：吸收首次启动的磁盘缓存/WebView2 初始化噪声，不计时
$warm = Start-Process -FilePath $exePath -PassThru
$null = Wait-MainWindowShown -Proc $warm -TimeoutMs $StabilityTimeoutMs
Stop-AppGracefully -Proc $warm
if (-not $warm.HasExited) { Write-Warning '预热启动未能正常退出，后续测量可能受影响' }

$coldSamples = @()
for ($i = 1; $i -le $ColdStartRuns; $i++) {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $exePath -PassThru
  $shown = Wait-MainWindowShown -Proc $p -TimeoutMs $StabilityTimeoutMs
  $sw.Stop()
  if ($shown -and -not $p.HasExited) {
    $coldSamples += [math]::Round($sw.Elapsed.TotalMilliseconds, 0)
    Write-Host ("coldStart 采样 {0}/{1}: {2} ms" -f $i, $ColdStartRuns, $coldSamples[-1])
  } else {
    Write-Warning ("coldStart 采样 {0}/{1} 失败（{2} 内窗口未出现或进程退出）" -f $i, $ColdStartRuns, $StabilityTimeoutMs)
  }
  Stop-AppGracefully -Proc $p
  Start-Sleep -Milliseconds 300
}
# 近似值注明：主窗口句柄出现 ≠ 应用可交互；真实打点见头注释
$coldStartMs = $null
if ($coldSamples.Count -gt 0) {
  $sortedSamples = @($coldSamples | Sort-Object)
  $coldStartMs = [double]$sortedSamples[[int][math]::Floor(($sortedSamples.Count - 1) / 2)]
}
if ($null -eq $coldStartMs) {
  Write-Warning '所有冷启动采样均失败，coldStartMs 置 null（compare.ps1 会跳过该指标）'
}

# ---- residualProcs（骨架） ------------------------------------------------
# 快照 diff：before -> N 次"启动-关闭" -> after；残留 = after 有而 before 没有的进程
$before = Get-AppSnapshot
Write-Host ("residual: 开关前 {0} 个 {1} 进程，开始 {2} 次启动-关闭循环" -f $before.Count, $AppName, $Toggles)

for ($i = 1; $i -le $Toggles; $i++) {
  $p = Start-Process -FilePath $exePath -PassThru
  $null = Wait-MainWindowShown -Proc $p -TimeoutMs $StabilityTimeoutMs
  Stop-AppGracefully -Proc $p
  Start-Sleep -Milliseconds 200   # 给句柄回收留一点时间
}

$after = Get-AppSnapshot
$residual = @($after | Where-Object { $before -notcontains $_ })
$residualProcs = $residual.Count
if ($residualProcs -gt 0) {
  Write-Warning ("检测到 {0} 个残留进程: {1}" -f $residualProcs, (($residual | ForEach-Object { "$_($AppName)" }) -join ', '))
} else {
  Write-Host "residual: 无残留进程"
}

# ---- 输出 -----------------------------------------------------------------
# trayRssMb / activeRssMb / warmStartMs：本阶段无法真测，置 null（接入时机见头注释）
$metrics = [ordered]@{
  trayRssMb     = $null
  activeRssMb   = $null
  coldStartMs   = $coldStartMs
  warmStartMs   = $null
  residualProcs = $residualProcs
}

$outPath = Resolve-CiPath $Out
$outDir = Split-Path -Parent $outPath
if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir -Force | Out-Null }
$json = $metrics | ConvertTo-Json
Set-Content -Path $outPath -Value $json -Encoding utf8NoBOM
Write-Host "指标已写入 $outPath"
Get-Content -Path $outPath -Raw | Write-Host
