# Compile-only check for background agents on the owner's Windows machine.
#
# Runs `cargo check`, `cargo clippy` or `dotnet build` on four cores at the Idle priority class,
# and only one at a time across every worktree, so the owner's own work keeps the rest of the
# machine. The script exits with the tool's own exit code, so it is 0 only when the check passed.
# Nothing else that builds or tests may run on this machine (AGENTS.md, Background agents).
#
#   pwsh scripts/compile-check.ps1 cargo-check  dsa/host/core/Cargo.toml
#   pwsh scripts/compile-check.ps1 cargo-clippy dsa/host/core/Cargo.toml -Extra '--all-targets','--','-D','warnings'
#   pwsh scripts/compile-check.ps1 dotnet-build dsa/host/platform/windows/settings/DsaSettings.sln
param(
    [Parameter(Mandatory, Position = 0)]
    [ValidateSet('cargo-check', 'cargo-clippy', 'dotnet-build')]
    [string]$Kind,
    [Parameter(Mandatory, Position = 1)]
    [string]$Path,
    [string[]]$Extra = @()
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Path)) { throw "no such manifest or project: $Path" }
$target = (Resolve-Path -LiteralPath $Path).Path

# One lock for all worktrees: it lives in the main checkout's .scratch, found through git's
# common directory, which every worktree shares.
$common = (git -C (Split-Path $target) rev-parse --path-format=absolute --git-common-dir).Trim()
$scratch = Join-Path (Split-Path $common) '.scratch'
New-Item -ItemType Directory -Force -Path $scratch | Out-Null
$lockPath = Join-Path $scratch 'compile-check.lock'

$lock = $null
$waited = $false
while (-not $lock) {
    try {
        $lock = [System.IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
    } catch [System.IO.IOException] {
        if (-not $waited) { Write-Host 'Another compile check is running; waiting for it.'; $waited = $true }
        Start-Sleep -Seconds 5
    }
}

try {
    $quoted = ($Extra | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }) -join ' '
    switch ($Kind) {
        'cargo-check'  { $tool = 'cargo'; $arguments = "check -j 4 --manifest-path `"$target`" $quoted" }
        'cargo-clippy' { $tool = 'cargo'; $arguments = "clippy -j 4 --manifest-path `"$target`" $quoted" }
        # No compiler server and no MSBuild node may outlive the build: a lingering
        # VBCSCompiler.exe or MSBuild node keeps running after the check, outside its four
        # cores, and serves the next build at whatever priority it was started with.
        'dotnet-build' {
            $tool = 'dotnet'
            $arguments = "build `"$target`" -m:4 -nologo -p:UseSharedCompilation=false -nodeReuse:false $quoted"
        }
    }
    $env:MSBUILDDISABLENODEREUSE = '1'
    $executable = (Get-Command $tool -CommandType Application | Select-Object -First 1).Source

    # The Idle priority class goes on this script first. A process started without a priority
    # flag inherits Idle from its parent, so the tool and every compiler it starts are Idle
    # from their first instruction, and anything the owner runs is scheduled first.
    # The tool is started directly rather than through `cmd /c start`, because `start` does not
    # hand the tool's exit code back, and a failed check would then report success.
    [System.Diagnostics.Process]::GetCurrentProcess().PriorityClass = 'Idle'
    $process = Start-Process -FilePath $executable -ArgumentList $arguments -NoNewWindow -PassThru
    # Reading the handle now keeps it open, so the exit code is still readable after the exit.
    $null = $process.Handle
    try { $process.PriorityClass = 'Idle' } catch { }
    # Only the tool itself is waited for. A wait on the whole process tree never returns while
    # the build leaves a descendant behind (a compiler server, VCTIP.EXE), and the lock would
    # then be held forever, queueing every other worktree's check behind it.
    $process.WaitForExit()
    $code = $process.ExitCode
    if ($null -eq $code) {
        Write-Host "$tool reported no exit code; the check counts as failed."
        $code = 1
    } elseif ($code -ne 0) {
        Write-Host "$tool exited with code $code; the check failed."
    }
    exit $code
} finally {
    $lock.Dispose()
}
