param([string]$BinDir, [string]$Results, [int]$Reps, [string[]]$Workloads, [string]$Python)
$ErrorActionPreference = 'Stop'
$Workloads = @($Workloads | ForEach-Object { $_ -split ',' })
Add-Type -Namespace '' -Name Psapi -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct PROCESS_MEMORY_COUNTERS_EX {
    public uint cb; public uint PageFaultCount;
    public UIntPtr PeakWorkingSetSize; public UIntPtr WorkingSetSize;
    public UIntPtr QuotaPeakPagedPoolUsage; public UIntPtr QuotaPagedPoolUsage;
    public UIntPtr QuotaPeakNonPagedPoolUsage; public UIntPtr QuotaNonPagedPoolUsage;
    public UIntPtr PagefileUsage; public UIntPtr PeakPagefileUsage; public UIntPtr PrivateUsage;
}
[DllImport("psapi.dll", SetLastError = true)]
public static extern bool GetProcessMemoryInfo(IntPtr hProcess, out PROCESS_MEMORY_COUNTERS_EX counters, uint size);
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool GetProcessTimes(IntPtr hProcess, out long creation, out long exit, out long kernel, out long user);
'@
if ($env:VARIANTS) {
  New-Item -ItemType Directory -Force $BinDir | Out-Null
  foreach ($v in $env:VARIANTS.Split(' ')) {
    $name = $v.Split('=')[0]
    $buildArgs = @('build', '--release', '--locked', '--no-default-features')
    if ($v -ne 'system') { $buildArgs += @('--features', $v.Split('=')[-1]) }
    Write-Host "cargo $($buildArgs -join ' ')"
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
    Copy-Item 'target/release/proxy-scraper-checker.exe' (Join-Path $BinDir "$name.exe")
  }
  & openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 1 -nodes -subj '/CN=127.0.0.1' *> $null
}
if (-not (Test-Path $Results)) {
  Set-Content -Path $Results -Encoding ascii -Value "label`tthp`tworkload`tvariant`trep`twall_ms`tuser_ms`tsys_ms`tmaxrss_kb`tfootprint_kb"
}
$tls = Start-Process -FilePath $Python -PassThru -NoNewWindow -ArgumentList @('bench/tls_server.py', '--port', '18443', '--cert', 'cert.pem', '--key', 'key.pem', '--count-file', 'hs.txt') -RedirectStandardOutput tls.log -RedirectStandardError tls.err
Start-Sleep -Seconds 2
$bins = Get-ChildItem $BinDir -Filter *.exe
try {
  for ($rep = 0; $rep -le $Reps; $rep++) {
    foreach ($wl in $Workloads) {
      $env:PROXY_SCRAPER_CHECKER_CONFIG = "bench/config.$wl.toml"
      foreach ($b in ($bins | Get-Random -Count $bins.Count)) {
        $psi = New-Object System.Diagnostics.ProcessStartInfo $b.FullName
        $psi.UseShellExecute = $false
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $p = [Diagnostics.Process]::Start($psi)
        $h = $p.Handle
        $outTask = $p.StandardOutput.ReadToEndAsync()
        $errTask = $p.StandardError.ReadToEndAsync()
        $p.WaitForExit()
        $sw.Stop()
        if ($p.ExitCode -ne 0) { Write-Host $outTask.Result $errTask.Result; throw "$($b.Name) $wl exit $($p.ExitCode)" }
        $c = New-Object PROCESS_MEMORY_COUNTERS_EX
        $c.cb = [Runtime.InteropServices.Marshal]::SizeOf($c)
        [void][Psapi]::GetProcessMemoryInfo($h, [ref]$c, $c.cb)
        $cr = 0L; $ex = 0L; $k = 0L; $u = 0L
        [void][Psapi]::GetProcessTimes($h, [ref]$cr, [ref]$ex, [ref]$k, [ref]$u)
        if ($rep -eq 0) { continue }
        $line = "$($env:LABEL)`tna`t$wl`t$($b.BaseName)`t$rep`t$($sw.ElapsedMilliseconds)`t$([int]($u / 10000))`t$([int]($k / 10000))`t$([int]($c.PeakWorkingSetSize.ToUInt64() / 1024))`t$([int]($c.PeakPagefileUsage.ToUInt64() / 1024))"
        Add-Content -Path $Results -Value $line -Encoding ascii
      }
    }
    Write-Host "rep $rep done"
  }
}
finally { $tls.Kill() }
