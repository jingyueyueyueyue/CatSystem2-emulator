param(
    [Parameter(Mandatory = $true)]
    [string]$Checkout
)

$ErrorActionPreference = 'Stop'
$workspace = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$engine = Join-Path $workspace 'simulator'
$destination = (Resolve-Path -LiteralPath $Checkout).Path
if ((Split-Path -Leaf $destination) -ne 'CatSystem2-emulator' -or
    $destination.StartsWith($workspace + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Select the separate CatSystem2-emulator Git checkout.'
}
$gitRoot = git -C $destination rev-parse --show-toplevel
if ($LASTEXITCODE -ne 0 -or (Resolve-Path -LiteralPath $gitRoot).Path -ne $destination) {
    throw 'The destination must be the root of a Git checkout.'
}
$changes = git -C $destination status --porcelain --untracked-files=no
if ($LASTEXITCODE -ne 0 -or $changes) {
    throw 'Review or commit existing tracked changes in the public checkout before syncing.'
}

$files = git -C $engine -c core.quotepath=false ls-files --cached --others --exclude-standard -- Cargo.toml Cargo.lock THIRD_PARTY.md crates android vendor/ttf-parser-0.21.1
if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate engine source.' }
$paths = @($files | Where-Object {
    ($_ -notmatch '(^|/)(\.git|\.gradle|\.cxx|\.externalNativeBuild|build|target|assets|jniLibs|__pycache__)(/|$)') -and
    ($_ -notmatch '(^|/)(local\.properties|.*\.user\.properties)$') -and
    ($_ -match '\.(rs|toml|c|h|js|java|xml|gradle|properties|md|txt|lock|orig|manifest)$|(^|/)LICENSE(?:-[A-Z0-9.]+)+$')
} | Sort-Object -Unique | ForEach-Object { "simulator/$_" })
$paths += 'scripts/build/android_workspace.py', 'scripts/build/prepare_android_ffmpeg.py'
$paths += 'scripts/analysis/android_load_probe.py', 'scripts/analysis/android_surface_metrics.py',
    'scripts/analysis/android_phase_metrics.py', 'scripts/analysis/android_flip_metrics.py',
    'scripts/analysis/run_playthrough_batch.py'

foreach ($path in $paths) {
    $source = Join-Path $workspace $path
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing source: $path" }
    $file = Get-Item -LiteralPath $source -Force
    if ($file.Length -gt 2MB -or $file.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Unexpected source type or size: $path"
    }
    if ($file.Name -notmatch '^(LICENSE|.*\.lock$)' -and
        (Select-String -LiteralPath $source -Quiet -Pattern '-----BEGIN .*PRIVATE KEY-----|\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}')) {
        throw "Potential credential: $path"
    }
}

foreach ($path in $paths) {
    $target = Join-Path $destination $path
    $parent = Split-Path -Parent $target
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    Copy-Item -LiteralPath (Join-Path $workspace $path) -Destination $target
}

$publicFiles = git -C $destination ls-files -- simulator scripts
$removed = @($publicFiles | Where-Object { $_ -notin $paths })
if ($removed.Count) {
    Write-Warning ('Previously published files absent from the current allowlist; review before staging: ' + ($removed -join ', '))
}
Write-Output "Synced $($paths.Count) source files. Review and stage the public changes separately."
