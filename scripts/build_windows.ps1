# Build the Windows release executable and copy it to dist/.
#
#   powershell -ExecutionPolicy Bypass -File scripts\build_windows.ps1 [-Wgpu]
#
# Output: dist\CcooNverter_v<version>_windows_x64.exe and dist\SHA256SUMS.txt
# (version read from the workspace Cargo.toml). Works in Windows PowerShell 5.1.

param(
    # Build with the wgpu renderer instead of the default OpenGL (glow) one.
    [switch]$Wgpu
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Version from [workspace.package] in Cargo.toml (first `version = "..."` line).
$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if ($null -eq $versionLine) { throw 'Could not read the version from Cargo.toml' }
$version = $versionLine.Matches[0].Groups[1].Value
Write-Host "CcooNverter v$version"

$cargoArgs = @('build', '--release', '-p', 'ccoonverter')
if ($Wgpu) { $cargoArgs += @('--features', 'wgpu') }
Write-Host "cargo $($cargoArgs -join ' ')"
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

$exe = Join-Path $root 'target\release\CcooNverter.exe'
if (-not (Test-Path $exe)) { throw "Build output not found: $exe" }

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$name = "CcooNverter_v${version}_windows_x64.exe"
$target = Join-Path $dist $name
Copy-Item -Force $exe $target

# sha256sum-compatible line: "<hex>  <file name>"
$hash = (Get-FileHash -Algorithm SHA256 $target).Hash.ToLowerInvariant()
$line = "$hash  $name"
$sums = Join-Path $dist 'SHA256SUMS.txt'
[System.IO.File]::WriteAllText($sums, "$line`n", (New-Object System.Text.UTF8Encoding($false)))

$size = (Get-Item $target).Length
Write-Host ("OK: {0} ({1:N0} bytes, {2:N1} MiB)" -f $target, $size, ($size / 1MB))
Write-Host "    $line"
