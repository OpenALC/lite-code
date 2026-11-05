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

  测量实现（P1，均在本机 release 产物上真测）：
  - coldStartMs    预热 1 次后连续启动 ColdStartRuns 次取中位数：计时起点 =
                   Start-Process 之前；终点 = 主窗口（按标题 "Lite Code" 定位
                   HWND）首次可见。真正的"冷启动到可交互"仍须等应用内埋点
                   （启动耗时经 lc-proto 事件打到 stdout），当前为近似值。
  - trayRssMb      启动应用 -> 主窗口关闭（Lifecycle 的 CloseRequested 走隐藏）
                   -> 静置 TraySettleSeconds（默认 10s，对齐 PLAN §4）-> 采样
                   "本应用进程 + 其全部子进程（WebView2）"的 WorkingSet 合计。
                   注意：当前 Lifecycle 关闭只 hide、不销毁 WebView，进程树仍完整
                   存活，因此该值不等于 PLAN 的"窗口已销毁"目标口径——指标如实
                   反映实现，不用改 app 代码去凑数。
  - warmStartMs    主窗口已隐藏时启动第二个实例；计时起点 = 二次 Start-Process
                   之前，终点 = 主窗口重新可见（单实例插件回调 show_main_window）。
                   取 WarmStartRuns 次的中位数；二次实例必须自行退出，否则 warn
                   并强杀（计入开关失败）。
  - residualProcs  before = 生命周期测量实例的主 PID；执行 Toggles 次
                   "关闭窗口（隐藏）-> 二次启动（唤回）" 循环后取进程快照 diff：
                   after 中出现且不属于主实例的 lc-desktop 进程数，加上循环中
                   "未自行退出" 的二次实例（它们会被立即强杀以免污染后续开关，
                   但计入残留，否则清理动作会把互斥失败掩盖成 0）。
  - activeRssMb    null。PLAN §4 要求"打开 3 文件 + 终端"的脚本化场景，当前应用
                   没有可供外部脚本驱动的 UI 自动化入口（无 IPC/命令行场景开关），
                   在 Lifecycle/Editor UI 提供该入口前无法真测；不伪造近似值。

  测量口径备注：
  - RSS 用 WorkingSet64（含共享页，逐进程求和会重复计算共享内存），是上限估计。
  - 主窗口"关闭"通过 PostMessage(WM_CLOSE) 触发，等价于用户点关闭按钮。
  - 本 harness 结束时强杀被测进程树（应用没有可脚本化的退出入口；托盘"退出"
    无法由脚本点击）。强杀只发生在测量结束后，不影响任何指标。
  - 若启动前已有 lc-desktop 进程，直接 exit 2：单实例会把已有进程当主实例，
    测量结果无意义（请先退出应用，或清理上次残留）。
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
  [int]$StabilityTimeoutMs = 15000,

  # 热启动采样次数（取中位数）
  [ValidateRange(1, 20)]
  [int]$WarmStartRuns = 3,

  # 关闭主窗口后到采样托盘 RSS 的静置秒数（PLAN §4：销毁后 10s 采样）
  [ValidateRange(1, 60)]
  [int]$TraySettleSeconds = 10
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$AppName = 'lc-desktop'   # 进程名（无 .exe）；WebView2 子进程经进程树包含
$WindowTitle = 'Lite Code'   # 主窗口标题（tauri.conf.json 的 productName）

# 用户态窗口操作：PostMessage(WM_CLOSE) 等价用户点关闭；IsWindowVisible 判定
# 主窗口可见性。
# 注意：Process.MainWindowHandle 不能用来判断主窗口——窗口隐藏后 .NET 会指向
# 进程里另一个可见的顶层辅助窗口（如单实例插件的 "dev.litecode.desktop-siw"），
# 造成"关闭后仍可见"的误判。这里按标题定位主窗口并缓存其 HWND。
if (-not ('LiteCodePerf.Win32' -as [type])) {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

namespace LiteCodePerf {
  public static class Win32 {
    private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool IsWindow(IntPtr hWnd);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    // 返回指定进程里标题匹配且可见的顶层窗口；没有则 Zero
    public static IntPtr FindVisibleWindow(int processId, string title) {
      IntPtr result = IntPtr.Zero;
      EnumWindows((h, l) => {
        uint pid;
        GetWindowThreadProcessId(h, out pid);
        if (pid != (uint)processId) return true;
        if (!IsWindowVisible(h)) return true;
        var sb = new StringBuilder(512);
        if (GetWindowText(h, sb, sb.Capacity) == 0) return true;
        if (sb.ToString() != title) return true;
        result = h;
        return false;
      }, IntPtr.Zero);
      return result;
    }
  }
}
'@
}
$WM_CLOSE = 0x0010

function Resolve-CiPath([string]$p) {
  # 相对路径按当前目录解析，绝对路径原样返回
  if ([System.IO.Path]::IsPathRooted($p)) { return $p }
  return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $p))
}

function Get-AppMainWindow {
  param([int]$ProcessId, [IntPtr]$CachedHandle, [string]$ExpectedTitle)
  # 返回"当前可见"的主窗口句柄；隐藏或已销毁时返回 Zero。
  # 缓存句柄仍有效时只看它的可见性（避免误选辅助窗口）；句柄被销毁
  # （Lifecycle 改为销毁/重建窗口）时按标题找回重建后的主窗口。
  if ($CachedHandle -ne [IntPtr]::Zero -and [LiteCodePerf.Win32]::IsWindow($CachedHandle)) {
    if ([LiteCodePerf.Win32]::IsWindowVisible($CachedHandle)) { return $CachedHandle }
    return [IntPtr]::Zero
  }
  return [LiteCodePerf.Win32]::FindVisibleWindow($ProcessId, $ExpectedTitle)
}

function Wait-MainWindowShown {
  param([int]$ProcessId, [IntPtr]$CachedHandle, [string]$ExpectedTitle, [int]$TimeoutMs)
  # 轮询主窗口；可见即返回其句柄（冷/热启动共用同一判据）
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
    $hwnd = Get-AppMainWindow -ProcessId $ProcessId -CachedHandle $CachedHandle -ExpectedTitle $ExpectedTitle
    if ($hwnd -ne [IntPtr]::Zero) { return $hwnd }
    if (-not (Test-ProcessAlive -ProcessId $ProcessId)) { return [IntPtr]::Zero }
    Start-Sleep -Milliseconds 25
  }
  return [IntPtr]::Zero
}

function Wait-AppWindowHidden {
  param([int]$ProcessId, [IntPtr]$CachedHandle, [string]$ExpectedTitle, [int]$TimeoutMs)
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
    if ((Get-AppMainWindow -ProcessId $ProcessId -CachedHandle $CachedHandle -ExpectedTitle $ExpectedTitle) -eq [IntPtr]::Zero) {
      return $true
    }
    Start-Sleep -Milliseconds 25
  }
  return $false
}

function Close-AppWindow {
  param([int]$ProcessId, [IntPtr]$CachedHandle, [string]$ExpectedTitle, [int]$TimeoutMs = 3000)
  # WM_CLOSE -> Lifecycle 的 CloseRequested -> 隐藏窗口（当前实现不销毁）
  $hwnd = Get-AppMainWindow -ProcessId $ProcessId -CachedHandle $CachedHandle -ExpectedTitle $ExpectedTitle
  if ($hwnd -eq [IntPtr]::Zero) { return $true }   # 已隐藏/已销毁
  if (-not [LiteCodePerf.Win32]::PostMessage($hwnd, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)) {
    Write-Warning ("PostMessage(WM_CLOSE) 失败（pid={0}, hwnd={1}）" -f $ProcessId, $hwnd)
  }
  return (Wait-AppWindowHidden -ProcessId $ProcessId -CachedHandle $CachedHandle -ExpectedTitle $ExpectedTitle -TimeoutMs $TimeoutMs)
}

function Get-ProcessTreeIds {
  param([int]$RootId)
  # 根进程 + 全部后代（WebView2 的 browser/renderer/gpu 都是根进程的后代）
  $children = @{}
  foreach ($p in (Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId -ErrorAction SilentlyContinue)) {
    $parent = [int]$p.ParentProcessId
    $child = [int]$p.ProcessId
    if (-not $children.ContainsKey($parent)) {
      $children[$parent] = [System.Collections.Generic.List[int]]::new()
    }
    $children[$parent].Add($child)
  }
  $ids = [System.Collections.Generic.List[int]]::new()
  $seen = [System.Collections.Generic.HashSet[int]]::new()
  $queue = [System.Collections.Generic.Queue[int]]::new()
  $queue.Enqueue($RootId)
  while ($queue.Count -gt 0) {
    $id = $queue.Dequeue()
    if (-not $seen.Add($id)) { continue }
    $ids.Add($id)
    if ($children.ContainsKey($id)) {
      foreach ($child in $children[$id]) { $queue.Enqueue($child) }
    }
  }
  return $ids
}

function Get-TreeRss {
  param([int]$RootId)
  # 进程树 RSS 合计（WorkingSet64；含共享页，逐进程求和会重复计共享内存）
  $totalBytes = [int64]0
  $parts = [System.Collections.Generic.List[object]]::new()
  foreach ($id in (Get-ProcessTreeIds -RootId $RootId)) {
    try {
      $proc = Get-Process -Id $id -ErrorAction Stop
    } catch {
      continue
    }
    $ws = [int64]$proc.WorkingSet64
    $totalBytes += $ws
    $parts.Add([pscustomobject]@{ Id = $id; Name = $proc.ProcessName; RssMb = [math]::Round($ws / 1MB, 1) })
  }
  return [pscustomobject]@{
    TotalMb = [math]::Round($totalBytes / 1MB, 1)
    Parts   = $parts
  }
}

function Test-ProcessAlive {
  param([int]$ProcessId)
  # 已终止但仍有句柄未关闭的进程（zombie）仍会出现在 Get-Process 里，
  # 必须用 HasExited 区分，否则会把"已退出"误判为"残留"
  $proc = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
  if ($null -eq $proc) { return $false }
  try { return -not $proc.HasExited } catch { return $false }
}

function Stop-ProcessTree {
  param([int]$RootId, [int]$TimeoutMs = 5000)
  # 深度优先倒序强杀（先子后父），只杀本次捕获的 PID，避免误伤其他 WebView2 应用
  $ids = @(Get-ProcessTreeIds -RootId $RootId)
  [array]::Reverse($ids)
  foreach ($id in $ids) {
    Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
  }
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
    $alive = @($ids | Where-Object { Test-ProcessAlive -ProcessId $_ })
    if ($alive.Count -eq 0) { return $true }
    Start-Sleep -Milliseconds 100
  }
  return $false
}

function Get-MedianMs {
  param([double[]]$Samples)
  # 偶数个样本取下中位数（与旧实现一致，避免额外引入插值语义）
  if (-not $Samples -or $Samples.Count -eq 0) { return $null }
  $sorted = @($Samples | Sort-Object)
  return [double]$sorted[[int][math]::Floor(($sorted.Count - 1) / 2)]
}

function Invoke-SecondInstanceWake {
  param([string]$ExePath, [int]$MainPid, [IntPtr]$CachedHandle, [string]$ExpectedTitle, [int]$TimeoutMs)
  # 隐藏窗口状态下启动第二实例，测主窗口重新可见的耗时
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $second = Start-Process -FilePath $ExePath -PassThru
  $hwnd = Wait-MainWindowShown -ProcessId $MainPid -CachedHandle $CachedHandle -ExpectedTitle $ExpectedTitle -TimeoutMs $TimeoutMs
  $sw.Stop()
  $ms = if ($hwnd -ne [IntPtr]::Zero) { [double][math]::Round($sw.Elapsed.TotalMilliseconds, 0) } else { $null }
  return [pscustomobject]@{ Ms = $ms; Handle = $hwnd; Second = $second }
}

function Wait-SecondInstanceExit {
  param([System.Diagnostics.Process]$Proc, [int]$TimeoutMs = 8000)
  # 单实例插件应在唤回主窗口后立刻退出二次实例；不退 = 互斥/残留问题
  if ($Proc.HasExited) { return $true }
  return $Proc.WaitForExit($TimeoutMs)
}

# ---- 前置检查 -------------------------------------------------------------
$exePath = Resolve-CiPath $Exe
if (-not (Test-Path $exePath)) {
  Write-Error "未找到被测程序 $exePath；perf.yml 在本步之前会执行 cargo build --release，请先构建"
  exit 2
}
Write-Host "被测程序: $exePath"

$existing = @(Get-Process -Name $AppName -ErrorAction SilentlyContinue | Where-Object { -not $_.HasExited })
if ($existing.Count -gt 0) {
  Write-Error ("检测到已有 {0} 个 {1} 进程（PID: {2}）。单实例插件会把它们当作主实例，测量结果无效；请先退出应用或清理上次残留后重跑。" -f `
      $existing.Count, $AppName, (($existing | ForEach-Object { $_.Id }) -join ', '))
  exit 2
}

# ---- coldStartMs（近似） --------------------------------------------------
# 预热 1 次：吸收首次启动的磁盘缓存/WebView2 初始化噪声，不计时
$warm = Start-Process -FilePath $exePath -PassThru
$null = Wait-MainWindowShown -ProcessId $warm.Id -CachedHandle ([IntPtr]::Zero) -ExpectedTitle $WindowTitle -TimeoutMs $StabilityTimeoutMs
$null = Stop-ProcessTree -RootId $warm.Id
Start-Sleep -Milliseconds 500

$coldSamples = @()
for ($i = 1; $i -le $ColdStartRuns; $i++) {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $exePath -PassThru
  $shownHwnd = Wait-MainWindowShown -ProcessId $p.Id -CachedHandle ([IntPtr]::Zero) -ExpectedTitle $WindowTitle -TimeoutMs $StabilityTimeoutMs
  $sw.Stop()
  if ($shownHwnd -ne [IntPtr]::Zero -and -not $p.HasExited) {
    $coldSamples += [math]::Round($sw.Elapsed.TotalMilliseconds, 0)
    Write-Host ("coldStart 采样 {0}/{1}: {2} ms" -f $i, $ColdStartRuns, $coldSamples[-1])
  } else {
    Write-Warning ("coldStart 采样 {0}/{1} 失败（{2} ms 内窗口未出现或进程退出）" -f $i, $ColdStartRuns, $StabilityTimeoutMs)
  }
  # 应用没有脚本化退出入口（关闭 = 隐藏），测量结束后强杀整棵进程树
  $null = Stop-ProcessTree -RootId $p.Id
  Start-Sleep -Milliseconds 300
}
$coldStartMs = Get-MedianMs -Samples $coldSamples
if ($null -eq $coldStartMs) {
  Write-Warning '所有冷启动采样均失败，coldStartMs 置 null（compare.ps1 会跳过该指标）'
}

# ---- 常驻实例：托盘 RSS / 热启动 / 残留进程 ---------------------------------
$trayRssMb = $null
$warmStartMs = $null
$residualProcs = $null

$app = Start-Process -FilePath $exePath -PassThru
$mainPid = $app.Id
$mainHwnd = Wait-MainWindowShown -ProcessId $mainPid -CachedHandle ([IntPtr]::Zero) -ExpectedTitle $WindowTitle -TimeoutMs $StabilityTimeoutMs
if ($mainHwnd -eq [IntPtr]::Zero) {
  Write-Warning ("常驻测量实例未在 {0} ms 内显示主窗口，托盘/热启动/残留指标将不可靠" -f $StabilityTimeoutMs)
}
Start-Sleep -Milliseconds 1500   # 让 WebView2 初始化与首屏渲染稳定

# 托盘常驻：关闭主窗口 -> 静置 -> 采样进程树 RSS
if (Test-ProcessAlive -ProcessId $mainPid) {
  if (-not (Close-AppWindow -ProcessId $mainPid -CachedHandle $mainHwnd -ExpectedTitle $WindowTitle)) {
    Write-Warning 'WM_CLOSE 后主窗口仍可见（Lifecycle 隐藏路径可能未生效）'
  }
  Start-Sleep -Seconds $TraySettleSeconds
  if (Test-ProcessAlive -ProcessId $mainPid) {
    $rss = Get-TreeRss -RootId $mainPid
    $trayRssMb = $rss.TotalMb
    $mainPart = @($rss.Parts | Where-Object { $_.Id -eq $mainPid })[0]
    $childParts = @($rss.Parts | Where-Object { $_.Id -ne $mainPid })
    $childMb = 0
    if ($childParts.Count -gt 0) {
      $childMb = [math]::Round((($childParts | Measure-Object -Property RssMb -Sum).Sum), 1)
    }
    Write-Host ("trayRss: 进程树合计 {0} MB（主进程 {1} MB + {2} 个子进程 {3} MB；静置 {4}s）" -f `
        $trayRssMb, $mainPart.RssMb, $childParts.Count, $childMb, $TraySettleSeconds)
    if ($childParts.Count -gt 0) {
      Write-Host ("  WebView2/子进程: " + (($childParts | ForEach-Object { "{0}#{1} {2}MB" -f $_.Name, $_.Id, $_.RssMb }) -join ', '))
    }
  } else {
    Write-Warning '托盘常驻采样前主进程已退出（应用崩溃？）'
  }
} else {
  Write-Warning '常驻测量实例启动后立即退出，托盘 RSS 置 null'
}

# 热启动：窗口保持隐藏，二次启动 -> 主窗口重新可见
$warmSamples = @()
if (Test-ProcessAlive -ProcessId $mainPid) {
  for ($i = 1; $i -le $WarmStartRuns; $i++) {
    $null = Close-AppWindow -ProcessId $mainPid -CachedHandle $mainHwnd -ExpectedTitle $WindowTitle   # 上一轮唤回后窗口可见，先隐藏
    Start-Sleep -Milliseconds 300
    $wake = Invoke-SecondInstanceWake -ExePath $exePath -MainPid $mainPid -CachedHandle $mainHwnd -ExpectedTitle $WindowTitle -TimeoutMs $StabilityTimeoutMs
    if ($wake.Handle -ne [IntPtr]::Zero) { $mainHwnd = $wake.Handle }
    if ($null -ne $wake.Ms) {
      $warmSamples += $wake.Ms
      Write-Host ("warmStart 采样 {0}/{1}: {2} ms" -f $i, $WarmStartRuns, $wake.Ms)
    } else {
      Write-Warning ("warmStart 采样 {0}/{1} 失败：二次启动后主窗口未在 {2} ms 内重新可见" -f $i, $WarmStartRuns, $StabilityTimeoutMs)
    }
    if (-not (Wait-SecondInstanceExit -Proc $wake.Second)) {
      Write-Warning ("二次启动进程 {0} 未退出（单实例插件异常），强制结束" -f $wake.Second.Id)
      Stop-Process -Id $wake.Second.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 400
  }
  $warmStartMs = Get-MedianMs -Samples $warmSamples
  if ($null -eq $warmStartMs) {
    Write-Warning '所有热启动采样均失败，warmStartMs 置 null'
  }
} else {
  Write-Warning '主进程不可用，warmStartMs 置 null'
}

# 残留进程：Toggles 次"关闭窗口（隐藏）-> 二次启动（唤回）"
if (Test-ProcessAlive -ProcessId $mainPid) {
  $toggleFailures = 0
  $leakedSecondInstances = [System.Collections.Generic.List[int]]::new()
  Write-Host ("residual: 主实例 PID {0}，开始 {1} 次窗口开关循环" -f $mainPid, $Toggles)
  for ($i = 1; $i -le $Toggles; $i++) {
    if (-not (Close-AppWindow -ProcessId $mainPid -CachedHandle $mainHwnd -ExpectedTitle $WindowTitle)) {
      $toggleFailures++
      Write-Warning ("开关 {0}/{1}: 关闭窗口后仍可见" -f $i, $Toggles)
    }
    Start-Sleep -Milliseconds 120
    $wake = Invoke-SecondInstanceWake -ExePath $exePath -MainPid $mainPid -CachedHandle $mainHwnd -ExpectedTitle $WindowTitle -TimeoutMs $StabilityTimeoutMs
    if ($wake.Handle -ne [IntPtr]::Zero) { $mainHwnd = $wake.Handle }
    if ($null -eq $wake.Ms) {
      $toggleFailures++
      Write-Warning ("开关 {0}/{1}: 二次启动未唤回主窗口" -f $i, $Toggles)
    }
    if (-not (Wait-SecondInstanceExit -Proc $wake.Second)) {
      $toggleFailures++
      $leakedSecondInstances.Add($wake.Second.Id)
      Write-Warning ("开关 {0}/{1}: 二次启动进程 {2} 未退出，强制结束（计入残留）" -f $i, $Toggles, $wake.Second.Id)
      Stop-Process -Id $wake.Second.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 200
  }

  Start-Sleep -Milliseconds 500   # 给二次实例退出与句柄回收留时间
  $after = @(Get-Process -Name $AppName -ErrorAction SilentlyContinue |
      Where-Object { -not $_.HasExited } |
      Select-Object -ExpandProperty Id)
  $residual = @($after | Where-Object { $_ -ne $mainPid })
  $residualProcs = $residual.Count + $leakedSecondInstances.Count
  if ($residualProcs -gt 0) {
    Write-Warning ("检测到 {0} 个残留进程: {1}" -f $residualProcs, (($residual | ForEach-Object { "$_($AppName)" }) -join ', '))
  } else {
    Write-Host "residual: 无残留进程"
  }
  if ($toggleFailures -gt 0) {
    Write-Warning ("开关循环失败 {0}/{1} 次（详见上方 warn），残留结果仅覆盖成功的开关" -f $toggleFailures, $Toggles)
  }

  # 测量结束，清理被测进程树（应用无脚本化退出入口；强杀不参与任何指标）
  $null = Stop-ProcessTree -RootId $mainPid
  foreach ($residualPid in $residual) {
    Stop-Process -Id $residualPid -Force -ErrorAction SilentlyContinue
  }
} else {
  Write-Warning '主进程不可用，residualProcs 置 null'
}

$leftover = @(Get-Process -Name $AppName -ErrorAction SilentlyContinue | Where-Object { -not $_.HasExited })
if ($leftover.Count -gt 0) {
  Write-Warning ("清理后仍有 {0} 个 {1} 进程: {2}" -f $leftover.Count, $AppName, (($leftover | ForEach-Object { $_.Id }) -join ', '))
}

# ---- 输出 -----------------------------------------------------------------
# activeRssMb：无"3 文件 + 终端"UI 自动化入口，无法真测（见头注释）
$metrics = [ordered]@{
  trayRssMb     = $trayRssMb
  activeRssMb   = $null
  coldStartMs   = $coldStartMs
  warmStartMs   = $warmStartMs
  residualProcs = $residualProcs
}

$outPath = Resolve-CiPath $Out
$outDir = Split-Path -Parent $outPath
if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir -Force | Out-Null }
$json = $metrics | ConvertTo-Json
Set-Content -Path $outPath -Value $json -Encoding utf8NoBOM
Write-Host "指标已写入 $outPath"
Get-Content -Path $outPath -Raw | Write-Host

