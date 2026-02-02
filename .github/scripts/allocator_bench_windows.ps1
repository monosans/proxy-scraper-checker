$tokioOn = $env:TOKIO_MULTI_THREAD
$tokioFeature = if ($tokioOn -eq "true") { "tokio-multi-thread" } else { "" }

$allocators = @("system", "mimalloc_v2", "mimalloc_v3")

function Build-Features([string]$allocator, [string]$tokio) {
  $features = @()
  if ($allocator -ne "system") { $features += $allocator }
  if ($tokio) { $features += $tokio }
  return ($features -join ",")
}

function Run-One([string]$allocator, [string]$features) {
  $args = @("run", "--release", "--locked")
  if ($features) { $args += "--features"; $args += $features }

  $peak = 0
  $proc = Start-Process -FilePath "cargo" -ArgumentList $args -PassThru -NoNewWindow
  while (-not $proc.HasExited) {
    Start-Sleep -Milliseconds 200
    try {
      $p = Get-Process -Id $proc.Id -ErrorAction Stop
      $current = [math]::Max($p.WorkingSet64, $p.PeakWorkingSet64)
      if ($current -gt $peak) { $peak = $current }
    } catch { }
  }
  $peakKb = [math]::Floor($peak / 1kb)
  Add-Content -Path results.tsv -Value "$allocator`t$peakKb"
}

if (Test-Path results.tsv) { Remove-Item results.tsv -Force }
foreach ($allocator in $allocators) {
  $features = Build-Features $allocator $tokioFeature
  Run-One $allocator $features
}

$rows = Get-Content results.tsv | ForEach-Object {
  $parts = $_ -split "`t"
  [pscustomobject]@{
    Allocator = $parts[0]
    PeakKB = [int]$parts[1]
  }
}
$sorted = $rows | Sort-Object PeakKB
$best = $sorted | Select-Object -First 1

$summary = @()
$summary += "### $($env:PLATFORM_LABEL) (tokio-multi-thread=$tokioOn)"
$summary += ""
$summary += "| Allocator | Peak KB |"
$summary += "| --- | ---: |"
foreach ($row in $sorted) {
  $summary += "| $($row.Allocator) | $($row.PeakKB) |"
}
$summary += ""
$summary += "**Best:** $($best.Allocator) ($($best.PeakKB) KB)"
$summary -join "`n" | Add-Content $env:GITHUB_STEP_SUMMARY
