# ClawZ one-click install — Windows (PowerShell 5.1+)
#
# From a clone:
#   .\scripts\install.ps1
#   .\scripts\install.ps1 -Source
#   .\scripts\install.ps1 -WithWeb
#
# Remote one-liner (PowerShell):
#   irm https://raw.githubusercontent.com/improwyz/clawz/main/scripts/install.ps1 | iex
#
param(
    [switch]$Docker,
    [switch]$Source,
    [switch]$WithWeb,
    [string]$InstallDir = "$env:USERPROFILE\clawz",
    [string]$GatewayUrl = "http://127.0.0.1:3000",
    [string]$RepoUrl = "https://github.com/improwyz/clawz.git",
    [string]$Branch = "main"
)

$ErrorActionPreference = "Stop"

function Write-ClawzLog($msg) { Write-Host "[clawz] $msg" -ForegroundColor Cyan }
function Write-ClawzWarn($msg) { Write-Host "[clawz] $msg" -ForegroundColor Yellow }
function Write-ClawzErr($msg) { Write-Host "[clawz] $msg" -ForegroundColor Red }

function Test-RepoRoot($path) {
    return (Test-Path (Join-Path $path "Cargo.toml")) -and (Test-Path (Join-Path $path "docker-compose.yml"))
}

function Ensure-Repo {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    $parent = Split-Path -Parent $scriptDir
    if (Test-RepoRoot $parent) {
        Set-Location $parent
        return (Get-Location).Path
    }

    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw "Git is required. Install from https://git-scm.com/download/win"
    }

    if (Test-Path (Join-Path $InstallDir ".git")) {
        Write-ClawzLog "Updating existing install at $InstallDir"
        git -C $InstallDir fetch --depth 1 origin $Branch
        git -C $InstallDir checkout $Branch
        git -C $InstallDir pull --ff-only origin $Branch
    } else {
        Write-ClawzLog "Cloning ClawZ into $InstallDir"
        git clone --depth 1 --branch $Branch $RepoUrl $InstallDir
    }
    Set-Location $InstallDir
    return (Get-Location).Path
}

function Write-EnvFile {
    if (Test-Path ".env") {
        Write-ClawzLog ".env already exists — leaving unchanged"
        return
    }
    if (Test-Path ".env.example") {
        Copy-Item ".env.example" ".env"
        $jwt = -join ((48..57) + (97..102) | Get-Random -Count 64 | ForEach-Object { [char]$_ })
        $worker = -join ((48..57) + (97..102) | Get-Random -Count 48 | ForEach-Object { [char]$_ })
        (Get-Content ".env") `
            -replace "change-me-in-production", $jwt `
            -replace "change-me-worker-token", $worker |
            Set-Content ".env"
        Write-ClawzLog "Created .env from .env.example"
    }
}

function Wait-Gateway {
    Write-ClawzLog "Waiting for gateway at $GatewayUrl ..."
    for ($i = 1; $i -le 90; $i++) {
        try {
            Invoke-WebRequest -Uri "$GatewayUrl/health" -UseBasicParsing -TimeoutSec 3 | Out-Null
            return
        } catch {
            Start-Sleep -Seconds 2
        }
    }
    throw "Gateway did not become healthy in time"
}

function Show-Success($mode) {
    Write-Host @"

╔══════════════════════════════════════════════════════════════╗
║  ClawZ is running ($mode)                                    ║
╠══════════════════════════════════════════════════════════════╣
║  Gateway API    $GatewayUrl
║  Health         $GatewayUrl/health
║  OpenAPI        $GatewayUrl/api/v1/system/openapi
║  Rooms API      $GatewayUrl/api/v1/rooms
╠══════════════════════════════════════════════════════════════╣
║  Quick test:  curl $GatewayUrl/api/v1/system/health
║  Stop:        docker compose down
║  Docs:        README.md — "New features setup"
╚══════════════════════════════════════════════════════════════╝

"@ -ForegroundColor Green
}

function Install-DockerStack {
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        throw "Docker not found. Install Docker Desktop: https://docs.docker.com/desktop/setup/install/windows-install/"
    }
    docker info 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Docker daemon is not running. Start Docker Desktop and retry."
    }

    Write-EnvFile
    Write-ClawzLog "Building gateway, worker, and database..."
    docker compose build gateway worker
    Write-ClawzLog "Starting services..."
    docker compose up -d db
    docker compose up -d worker gateway
    Wait-Gateway
    Show-Success "Docker Compose"
}

function Install-FromSource {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "Rust/cargo not found. Install from https://rustup.rs"
    }
    Write-EnvFile
    Write-ClawzLog "Building release binaries..."
    cargo build --release -p clawz-gateway -p clawz-worker

    $env:CLAWZ_DISABLE_AUTH = "1"
    $env:CLAWZ_STUB_PROVIDER = "1"
    $env:CLAWZ_LISTEN_ADDR = "127.0.0.1:50051"
    Start-Process -FilePath ".\target\release\clawz-worker.exe" -WindowStyle Hidden
    Start-Sleep -Seconds 2

    $env:CLAWZ_JWT_SECRET = "local-dev-secret"
    $env:CLAWZ_WORKER_TOKEN = "local-dev-worker"
    $env:WORKER_URL = "http://127.0.0.1:50051"
    Start-Process -FilePath ".\target\release\clawz-gateway.exe" -WindowStyle Hidden

    Wait-Gateway
    Show-Success "source (local binaries)"
    Write-ClawzWarn "Stop processes from Task Manager or: Get-Process clawz-* | Stop-Process"
}

function Install-WebDashboard {
    if (-not (Test-Path "web")) {
        Write-ClawzWarn "web/ not found — skipping dashboard"
        return
    }
    if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
        throw "Node.js/npm required for dashboard. Install from https://nodejs.org/"
    }
    Push-Location web
    npm ci
    npm run build
    Pop-Location
    Write-ClawzLog "Dashboard built to web/dist"
}

Write-ClawzLog "ClawZ installer — Windows"
$root = Ensure-Repo
Write-ClawzLog "Using repository at $root"

$mode = if ($Source) { "source" } elseif ($Docker) { "docker" } else { "auto" }

if ($mode -eq "auto") {
    if (Get-Command docker -ErrorAction SilentlyContinue) {
        try {
            docker info 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) { $mode = "docker" } else { $mode = "source" }
        } catch {
            $mode = "source"
        }
    } else {
        $mode = "source"
        Write-ClawzWarn "Docker not available — falling back to source install"
    }
}

switch ($mode) {
    "docker" { Install-DockerStack }
    "source" { Install-FromSource }
}

if ($WithWeb) {
    Install-WebDashboard
}

Write-ClawzLog "Install complete."
