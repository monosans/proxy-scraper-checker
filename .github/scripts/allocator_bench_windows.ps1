# Allocator benchmark, Windows. Same contract and same TSV schema as
# allocator_bench_unix.sh.
#
# Fixes over the previous version:
#   - it polled WorkingSet64 every 100 ms and stopped the instant HasExited, so
#     a peak reached in the final tick was missed - and save_proxies is the
#     last thing the program does;
#   - the per-tick Get-CimInstance Win32_Process perturbed what it measured;
#   - Start-Process -PassThru without -Wait leaves .ExitCode empty, so a
#     panicking binary produced a "successful" row, and the cargo build's exit
#     code was discarded entirely;
#   - $args is an automatic variable and was being shadowed;
#   - no time was recorded at all.
# Peak working set, peak commit, page faults and CPU time now come from the
# kernel after exit through a retained process handle, which is exact.

$ErrorActionPreference = 'Stop'

$reps = if ($env:REPS) { [int]$env:REPS } else { 5 }
$warmups = if ($env:WARMUPS) { [int]$env:WARMUPS } else { 1 }
$workloads = if ($env:WORKLOADS) { $env:WORKLOADS -split ' ' } else { @('check', 'scrape') }
$allocEnvs = if ($env:ALLOC_ENVS) { $env:ALLOC_ENVS -split ';' } else { @('default') }
$runId = if ($env:RUN_ID) { $env:RUN_ID } else { '0' }
$platform = if ($env:PLATFORM_LABEL) { $env:PLATFORM_LABEL } else { 'unknown' }
$allocator = $env:ALLOCATOR
$mt = if ($env:TOKIO_MULTI_THREAD -eq 'true') { 'true' } else { 'false' }

# --- feature selection -----------------------------------------------------
# "system" and "auto" are not features. "system" is --no-default-features with
# nothing added; "auto" is the shipped default feature set, which nothing in
# the previous benchmark ever built even though it is what users actually get.
$features = @()
$noDefault = @('--no-default-features')
switch ($allocator) {
  'system' { }
  'auto' { $noDefault = @() }
  'mimalloc_v3_override' { $features += 'mimalloc_v3'; $features += 'mimalloc_override' }
  default { $features += $allocator }
}
if ($mt -eq 'true') { $features += 'tokio-multi-thread' }

$buildArgs = @('build', '--release', '--locked') + $noDefault
if ($features.Count -gt 0) { $buildArgs += @('--features', ($features -join ',')) }

Write-Host "cargo $($buildArgs -join ' ')"
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

$exe = (Resolve-Path 'target\release\proxy-scraper-checker.exe').Path

# --- kernel accounting through a retained handle ---------------------------
if (-not ('Psapi' -as [type])) {
  Add-Type -Namespace '' -Name Psapi -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct PROCESS_MEMORY_COUNTERS_EX {
    public uint cb;
    public uint PageFaultCount;
    public UIntPtr PeakWorkingSetSize;
    public UIntPtr WorkingSetSize;
    public UIntPtr QuotaPeakPagedPoolUsage;
    public UIntPtr QuotaPagedPoolUsage;
    public UIntPtr QuotaPeakNonPagedPoolUsage;
    public UIntPtr QuotaNonPagedPoolUsage;
    public UIntPtr PagefileUsage;
    public UIntPtr PeakPagefileUsage;
    public UIntPtr PrivateUsage;
}

[DllImport("psapi.dll", SetLastError = true)]
public static extern bool GetProcessMemoryInfo(IntPtr hProcess, out PROCESS_MEMORY_COUNTERS_EX counters, uint size);

[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool GetProcessTimes(IntPtr hProcess, out long creation, out long exit, out long kernel, out long user);
'@
}

$cores = $env:NUMBER_OF_PROCESSORS
$arch = $env:PROCESSOR_ARCHITECTURE
$toolchain = ((& rustc -V) -split ' ')[1]
$exeSha = (Get-FileHash -Algorithm SHA256 $exe).Hash.Substring(0, 16).ToLower()

$results = 'bench-results.tsv'
if (-not (Test-Path $results)) {
  $header = 'run_id', 'cell_id', 'rep', 'platform', 'libc', 'arch', 'cores',
  'toolchain', 'allocator', 'alloc_env', 'mt', 'workload', 'wall_ms',
  'cpu_user_ms', 'cpu_sys_ms', 'peak_rss_kb', 'peak_extra_kb',
  'peak_extra_kind', 'major_pf', 'minor_pf', 'pf_kind', 'exit_code',
  'proxies_checked', 'proxies_out', 'exe_sha', 'thp', 'page_kb'
  Set-Content -Path $results -Value ($header -join "`t") -Encoding utf8
}

function Get-Median([double[]]$v) {
  $s = @($v | Sort-Object)
  $n = $s.Count
  if ($n % 2) { return $s[[math]::Floor($n / 2)] }
  return ($s[$n / 2 - 1] + $s[$n / 2]) / 2
}
function Get-Mad([double[]]$v) {
  $m = Get-Median $v
  return Get-Median @($v | ForEach-Object { [math]::Abs($_ - $m) })
}

# --- tls workload support --------------------------------------------------
# Must match check_url in bench/config.tls.toml and the port baked into
# bench/corpus/tls_http.txt.
$tlsPort = 18443
$script:tlsProc = $null

function Stop-TlsServer {
  if ($script:tlsProc -and -not $script:tlsProc.HasExited) {
    try { $script:tlsProc.Kill() } catch { }
  }
  $script:tlsProc = $null
}

# Returns $false to SKIP the tls workload rather than fail the job: a missing
# python or openssl must not throw away this job's check and scrape rows.
function Start-TlsServer {
  $script:tlsProc = $null
  $py = $null
  foreach ($candidate in 'python', 'python3') {
    $cmd = Get-Command $candidate -ErrorAction SilentlyContinue
    # Skip the Windows Store execution-alias stubs, which exit without running.
    if ($cmd -and $cmd.Source -notlike '*WindowsApps*') { $py = $cmd.Source; break }
  }
  if (-not $py) {
    Write-Host '::warning::python not found - skipping the tls workload'
    return $false
  }
  if (-not (Get-Command openssl -ErrorAction SilentlyContinue)) {
    Write-Host '::warning::openssl not found - skipping the tls workload'
    return $false
  }

  if (-not (Test-Path 'bench-tls-cert.pem')) {
    # Self-signed on purpose: the client is meant to reject it, which is what
    # makes the outcome deterministic.
    & openssl req -x509 -newkey rsa:2048 -keyout bench-tls-key.pem `
      -out bench-tls-cert.pem -days 1 -nodes -subj '/CN=127.0.0.1' `
      *> bench-tls-openssl.log
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path 'bench-tls-cert.pem')) {
      Write-Host '::warning::openssl could not create a cert - skipping tls'
      if (Test-Path 'bench-tls-openssl.log') { Get-Content bench-tls-openssl.log | Write-Host }
      return $false
    }
  }

  if (Test-Path 'bench-tls-server.log') { Remove-Item bench-tls-server.log -Force }
  $script:tlsProc = Start-Process -FilePath $py -PassThru -NoNewWindow `
    -ArgumentList @('bench/tls_server.py', '--port', "$tlsPort",
      '--cert', 'bench-tls-cert.pem', '--key', 'bench-tls-key.pem',
      '--count-file', 'bench-tls-handshakes.txt') `
    -RedirectStandardOutput bench-tls-server.log `
    -RedirectStandardError bench-tls-server.err

  # Probe the port, not the log. Start-Process holds the redirected stdout file
  # open, and Select-String on it can fail to read while it is being written -
  # which loses the readiness signal, skips the workload and leaves that cell
  # with no rows. One windows-2025 baseline cell went missing exactly that way.
  # The probe closes immediately, so the server drops it before counting.
  for ($w = 0; $w -lt 150; $w++) {
    try {
      $probe = New-Object System.Net.Sockets.TcpClient
      $probe.Connect('127.0.0.1', $tlsPort)
      $connected = $probe.Connected
      $probe.Close()
      if ($connected) { return $true }
    }
    catch { }
    if ($script:tlsProc.HasExited) { break }
    Start-Sleep -Milliseconds 100
  }

  Write-Host '::warning::tls server never became ready - skipping the tls workload'
  foreach ($f in 'bench-tls-server.log', 'bench-tls-server.err') {
    if (Test-Path $f) { Get-Content $f | Write-Host }
  }
  Stop-TlsServer
  return $false
}

# Workloads and tuning variants are loops inside the job, not matrix axes:
# neither needs a rebuild, and the build is what costs minutes.
#
# allocator_bench_unix.sh interleaves the tuning variants across repetitions so
# that drift in the machine cannot land on whichever variant ran last. This
# script deliberately does not: allocator_bench_matrix.py only ever emits the
# variants for Linux allocators, so every Windows cell has exactly one
# ("default") and the interleaving would be a no-op.
foreach ($workload in $workloads) {
  if ($workload -eq 'tls' -and -not (Start-TlsServer)) { continue }

  foreach ($allocEnv in $allocEnvs) {
    $allocEnvName = $allocEnv.Split(':')[0]
    $cellId = "$platform|$allocator|mt=$mt|$workload|$allocEnvName"
    Write-Host "--- $cellId ---"
    $rows = @()

    for ($i = 1; $i -le ($warmups + $reps); $i++) {
      $psi = New-Object System.Diagnostics.ProcessStartInfo
      $psi.FileName = $exe
      $psi.UseShellExecute = $false
      $psi.RedirectStandardOutput = $true
      $psi.RedirectStandardError = $true
      $psi.WorkingDirectory = (Get-Location).Path
      $psi.EnvironmentVariables['PROXY_SCRAPER_CHECKER_CONFIG'] = "bench/config.$workload.toml"
      # mimalloc reads MIMALLOC_* at process load, so allocator tuning is a
      # run-time axis and needs no rebuild.
      if ($allocEnv -ne $allocEnvName) {
        foreach ($pair in ($allocEnv.Substring($allocEnvName.Length + 1) -split ' ')) {
          if ($pair) {
            $kv = $pair.Split('=', 2)
            $psi.EnvironmentVariables[$kv[0]] = $kv[1]
          }
        }
      }

      $sw = [System.Diagnostics.Stopwatch]::StartNew()
      $p = [System.Diagnostics.Process]::Start($psi)
      # Drain both pipes concurrently; reading one to completion first
      # deadlocks as soon as the other fills its buffer.
      $stdout = $p.StandardOutput.ReadToEndAsync()
      $stderr = $p.StandardError.ReadToEndAsync()
      $p.WaitForExit()
      $sw.Stop()

      $handle = $p.Handle
      $mem = New-Object Psapi+PROCESS_MEMORY_COUNTERS_EX
      $mem.cb = [System.Runtime.InteropServices.Marshal]::SizeOf($mem)
      if (-not [Psapi]::GetProcessMemoryInfo($handle, [ref]$mem, $mem.cb)) {
        throw "GetProcessMemoryInfo failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())"
      }
      $creation = 0; $exitT = 0; $kernel = 0; $user = 0
      if (-not [Psapi]::GetProcessTimes($handle, [ref]$creation, [ref]$exitT, [ref]$kernel, [ref]$user)) {
        throw "GetProcessTimes failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())"
      }

      $exitCode = $p.ExitCode
      $log = "bench-$workload-$allocEnvName-$i.log"
      Set-Content -Path $log -Value ($stdout.Result + $stderr.Result) -Encoding utf8
      $p.Dispose()

      if ($exitCode -ne 0) {
        Get-Content $log -Tail 40 | Write-Host
        throw "rep $i of $cellId exited with $exitCode"
      }

      # 100 ns ticks -> ms.
      $userMs = [int64]($user / 10000)
      $sysMs = [int64]($kernel / 10000)
      $wallMs = [int64]$sw.Elapsed.TotalMilliseconds
      $peakRssKb = [int64]([uint64]$mem.PeakWorkingSetSize / 1kb)
      $peakCommitKb = [int64]([uint64]$mem.PeakPagefileUsage / 1kb)

      # Work-done counters. A clean exit 0 that scraped nothing is the failure
      # mode the previous harness could not see.
      $checked = 0
      $m = [regex]::Match((Get-Content $log -Raw), 'Started checking (\d+) proxies')
      if ($m.Success) { $checked = [int]$m.Groups[1].Value }
      $outFile = 'bench-out\proxies\all.txt'
      $outN = if (Test-Path $outFile) { @(Get-Content $outFile).Count } else { 0 }

      # See the matching note in allocator_bench_unix.sh: config.rs clamps
      # max_concurrent_checks to the fd limit it managed to obtain, which would
      # change the workload without changing any recorded column.
      if ((Get-Content $log -Raw) -match 'max_concurrent_checks config value is too high') {
        Write-Host "::warning::$cellId rep ${i}: concurrency was clamped below the configured 512; this platform is not running the same workload"
      }

      # tls shares the check guard: every one of its 2000 proxies must reach
      # Proxy::check, and 0 means the corpus, the config or the local server is
      # wrong rather than that the allocator was fast.
      if (($workload -eq 'check' -or $workload -eq 'tls') -and $checked -eq 0) {
        Get-Content $log -Tail 40 | Write-Host
        throw "rep $i checked no proxies - corpus or config is wrong"
      }
      if ($workload -eq 'scrape' -and $outN -eq 0) {
        Get-Content $log -Tail 40 | Write-Host
        throw "rep $i wrote no proxies - corpus or config is wrong"
      }

      if ($i -le $warmups) { continue }

      # PageFaultCount is a single lifetime total that includes file-backed
      # faults; it is NOT comparable to the Linux/macOS minor-fault column,
      # hence the pf_kind discriminator.
      $row = @($runId, $cellId, ($i - $warmups), $platform, 'msvc', $arch,
        $cores, $toolchain, $allocator, $allocEnvName, $mt, $workload, $wallMs,
        $userMs, $sysMs, $peakRssKb, $peakCommitKb, 'win_peak_commit', 0,
        $mem.PageFaultCount, 'win_total', $exitCode, $checked, $outN, $exeSha,
        'n/a', 4) -join "`t"
      Add-Content -Path $results -Value $row -Encoding utf8
      $rows += [pscustomobject]@{
        wall = $wallMs; cpu = $userMs + $sysMs
        rss  = $peakRssKb; commit = $peakCommitKb
      }
    }

    # Median rather than mean, MAD rather than stddev: with 5 reps a single
    # descheduled run would drag a mean far enough to invent a difference.
    # Wrapped because the summary is cosmetic: bench-results.tsv is the
    # artifact the aggregate job reads, and a formatting bug must not discard
    # a cell that already ran to completion.
    try {
    $summary = @("### $cellId", '',
      "reps: $($rows.Count) &nbsp;&nbsp; cores: $cores &nbsp;&nbsp; rustc: $toolchain &nbsp;&nbsp; exe: $exeSha", '',
      '| metric | median | MAD |', '| --- | ---: | ---: |')
    foreach ($metric in @(
        @{ n = 'wall ms'; v = @($rows.wall) },
        @{ n = 'cpu ms (user+sys)'; v = @($rows.cpu) },
        @{ n = 'peak RSS KB'; v = @($rows.rss) },
        @{ n = 'peak commit KB'; v = @($rows.commit) })) {
      $summary += "| $($metric.n) | $([int](Get-Median $metric.v)) | $([int](Get-Mad $metric.v)) |"
    }
    $summary += ''
    ($summary -join "`n") | Add-Content -Path $env:GITHUB_STEP_SUMMARY -Encoding utf8
    } catch {
      Write-Host "::warning::summary failed for $cellId ($($_.Exception.Message)); results are still in $results"
    }
  }

  if ($workload -eq 'tls') {
    # See the matching note in allocator_bench_unix.sh: proxies_checked is
    # logged before the first check, so it cannot distinguish 2000 handshakes
    # from 2000 refused connects. The server counts, not the measured process.
    $handshakes = 0
    if (Test-Path 'bench-tls-handshakes.txt') {
      $handshakes = [int](Get-Content 'bench-tls-handshakes.txt' -Raw).Trim()
    }
    $expected = ($warmups + $reps) * 2000
    if ($handshakes -lt ($expected / 2)) {
      Write-Host "::warning::tls workload completed only $handshakes handshakes, expected about $expected - those rows measure failed connects, not TLS"
    }
    else {
      Write-Host "tls workload: $handshakes handshakes (expected ~$expected)"
    }
    Stop-TlsServer
  }
}

Stop-TlsServer
