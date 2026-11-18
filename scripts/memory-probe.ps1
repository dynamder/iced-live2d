# Measures the memory of one case: the native iced app, or a Chromium app window
# showing either the Cubism Web SDK page or a blank page.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/memory-probe.ps1 -Kind web
#
# Reported numbers per process tree:
#   private_ws  what Task Manager shows as "Memory" (private working set)
#   working_set all resident pages, including DLL pages shared with other processes
#   commit      private committed bytes

param(
    [Parameter(Mandatory = $true)][ValidateSet('web', 'blank', 'native', 'empty')][string]$Kind,
    [string]$Model = 'Hiyori',
    [int]$Warmup = 15,
    [int]$Samples = 5,
    [int]$Interval = 2,
    [string]$Url = '',
    [string]$Shot = ''
)

$repo = Split-Path -Parent $PSScriptRoot
$edgePath = 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
$label = @{ web = 'web sdk'; blank = 'blank page'; native = 'native app'; empty = 'native empty window' }[$Kind]

function Get-Tree([int]$root) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId
    $children = @{}
    foreach ($p in $all) {
        $parent = [int]$p.ParentProcessId
        if (-not $children.ContainsKey($parent)) { $children[$parent] = New-Object System.Collections.ArrayList }
        [void]$children[$parent].Add([int]$p.ProcessId)
    }

    $seen = New-Object System.Collections.Generic.HashSet[int]
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue($root)
    [void]$seen.Add($root)

    while ($queue.Count -gt 0) {
        $current = [int]$queue.Dequeue()
        if ($children.ContainsKey($current)) {
            foreach ($child in $children[$current]) {
                if ($seen.Add($child)) { $queue.Enqueue($child) }
            }
        }
    }

    return @($seen)
}

$server = $null
$app = $null

Get-CimInstance Win32_Process -Filter "Name='node.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -like '*web-probe\server.js*' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

try {
    if ($Kind -eq 'native') {
        $app = if ($Model -eq '') {
            Start-Process -FilePath "$repo\target\release\app.exe" -PassThru
        }
        else {
            Start-Process -FilePath "$repo\target\release\app.exe" -ArgumentList '--model', $Model -PassThru
        }
    }
    elseif ($Kind -eq 'empty') {
        $app = Start-Process -FilePath "$repo\target\release\examples\minimal.exe" -PassThru
    }
    else {
        $server = Start-Process -FilePath 'node' -ArgumentList "$repo\scripts\web-probe\server.js", '8731' -PassThru -WindowStyle Hidden
        Start-Sleep -Milliseconds 900

        $profile = "$repo\.tmp-web\profile-$Kind"
        if (Test-Path $profile) { Remove-Item -Recurse -Force $profile }

        $url = if ($Url -ne '') { $Url } elseif ($Kind -eq 'web') { 'http://127.0.0.1:8731/web/index.html' } else { 'http://127.0.0.1:8731/blank.html' }
        $app = Start-Process -FilePath $edgePath -ArgumentList @(
            "--user-data-dir=$profile",
            "--app=$url",
            '--no-first-run',
            '--no-default-browser-check',
            '--disable-extensions',
            '--disable-sync',
            '--disable-features=Translate,MediaRouter',
            '--window-size=680,740'
        ) -PassThru
    }

    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 500
        $window = Get-Process -Id $app.Id -ErrorAction SilentlyContinue
    } until (($window -and $window.MainWindowTitle -ne '' -and $window.MainWindowTitle -notmatch '^(127\.0\.0\.1|https?://|file://)') -or (Get-Date) -gt $deadline)

    if (-not $window -or $window.MainWindowTitle -eq '') { throw "no window appeared for pid $($app.Id)" }

    Write-Output ("window: " + $window.MainWindowTitle)

    & "$PSScriptRoot\shot-window.ps1" -Title $window.MainWindowTitle -Out "$repo\.tmp-web\raise.png" | Out-Null

    Start-Sleep -Seconds $Warmup

    if ($Shot -ne '') {
        & "$PSScriptRoot\shot-window.ps1" -Title $window.MainWindowTitle -Out $Shot
    }

    $rows = @()
    for ($i = 0; $i -lt $Samples; $i++) {
        if ($i -gt 0) { Start-Sleep -Seconds $Interval }

        $pids = Get-Tree $app.Id
        $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process
        $byId = @{}
        foreach ($entry in $perf) {
            if ($entry.IDProcess -ne 0) { $byId[[int]$entry.IDProcess] = $entry }
        }

        $private = 0.0
        $working = 0.0
        $commit = 0.0
        $alive = 0
        $parts = @()

        foreach ($id in $pids) {
            if (-not $byId.ContainsKey($id)) { continue }
            $entry = $byId[$id]
            $alive++
            $private += [double]$entry.WorkingSetPrivate
            $working += [double]$entry.WorkingSet
            $commit += [double]$entry.PrivateBytes
            $parts += ('{0}#{1}={2:N0}MB' -f $entry.Name, $id, ([double]$entry.WorkingSetPrivate / 1MB))
        }

        $rows += [pscustomobject]@{
            sample      = $i
            processes   = $alive
            private_ws  = [math]::Round($private / 1MB, 1)
            working_set = [math]::Round($working / 1MB, 1)
            commit      = [math]::Round($commit / 1MB, 1)
            parts       = ($parts -join '  ')
        }
    }

    $rows | Format-Table -AutoSize -Wrap | Out-String -Width 250 | Write-Output

    $stable = $rows | Where-Object { $_.sample -gt 0 }
    Write-Output ('{0}: private_ws median {1:N1} MB / max {2:N1} MB, working_set median {3:N1} MB, commit median {4:N1} MB, {5} processes' -f `
            $Kind,
        (($stable | Measure-Object private_ws -Average).Average),
        (($rows | Measure-Object private_ws -Maximum).Maximum),
        (($stable | Measure-Object working_set -Average).Average),
        (($stable | Measure-Object commit -Average).Average),
        $rows[0].processes)

    $rows | Export-Csv -Path "$repo\.tmp-web\mem-$Kind.csv" -NoTypeInformation
}
finally {
    if ($app) {
        $victims = Get-Tree $app.Id
        foreach ($id in $victims) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    }
    if ($server) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
}
