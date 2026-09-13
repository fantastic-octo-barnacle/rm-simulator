# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
param(
    [ValidateSet('red', 'blue')][string]$Team = 'red',
    [string]$Server = ''
)
$ErrorActionPreference = 'Stop'
try {
    $cachePath = Join-Path $PSScriptRoot 'server.txt'
    $defaultServer = '127.0.0.1:7700'
    if (Test-Path -LiteralPath $cachePath) {
        $cached = (Get-Content -LiteralPath $cachePath -Raw).Trim()
        if (-not [string]::IsNullOrWhiteSpace($cached)) { $defaultServer = $cached }
    }
    if ([string]::IsNullOrWhiteSpace($Server)) {
        $Server = Read-Host "Server IP or hostname [$defaultServer]"
        if ([string]::IsNullOrWhiteSpace($Server)) { $Server = $defaultServer }
    }
    $Server = $Server.Trim()
    if ($Server -notmatch '^[a-zA-Z0-9][a-zA-Z0-9.-]*(?::[0-9]{1,5})?$') {
        throw 'Enter an IPv4 address or hostname, optionally followed by :port.'
    }
    if ($Server -notmatch ':') { $Server += ':7700' }
    $port = [int]($Server.Split(':')[-1])
    if ($port -lt 1 -or $port -gt 65535) { throw 'Port must be between 1 and 65535.' }
    try {
        Set-Content -LiteralPath $cachePath -Value $Server -Encoding ASCII
    } catch {
        Write-Warning 'Could not save server.txt; continuing with the entered address.'
    }
    $computer = $env:COMPUTERNAME -replace '[^a-zA-Z0-9_-]', ''
    if ([string]::IsNullOrWhiteSpace($computer)) { $computer = 'pilot' }
    $name = $computer + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
    Write-Host "Joining $Server on $Team as $name"
    & (Join-Path $PSScriptRoot 'bin\rm-simulator.exe') --cad-assets (Join-Path $PSScriptRoot 'field') --connect $Server --team $Team --name $name
    if ($LASTEXITCODE -ne 0) { throw "Simulator exited with code $LASTEXITCODE" }
} catch {
    Write-Host $_ -ForegroundColor Red
}
Read-Host 'Press Enter to close'
