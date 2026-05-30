# ClawZ one-click install — Windows (PowerShell 5.1+)
#
# From a clone:
#   .\scripts\install.ps1
#   .\scripts\install.ps1 -Docker -Prebuilt
#   .\scripts\install.ps1 -Build
#   .\scripts\install.ps1 -BootstrapOnly
#   .\scripts\install.ps1 -InstallDocker
#   .\scripts\install.ps1 -WithWeb -Wizard
#
# Remote one-liner (PowerShell):
#   git clone --depth 1 https://github.com/improwyz/clawz.git $env:USERPROFILE\clawz; & "$env:USERPROFILE\clawz\scripts\install.ps1"
#
param(
    [switch]$Docker,
    [switch]$Source,
    [switch]$Build,
    [switch]$Prebuilt,
    [switch]$BootstrapOnly,
    [switch]$InstallDocker,
    [switch]$WithWeb,
    [switch]$Wizard,
    [string]$InstallDir = "$env:USERPROFILE\clawz",
    [string]$GatewayUrl = "http://127.0.0.1:3000",
    [string]$RepoUrl = "https://github.com/improwyz/clawz.git",
    [string]$Branch = "main",
    [string]$Registry = "ghcr.io/improwyz",
    [string]$Tag = "latest"
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
║  Web setup:   $GatewayUrl/setup
║  Docs:        INSTALL.md
╚══════════════════════════════════════════════════════════════╝

"@ -ForegroundColor Green
}

function Get-ComposeArgs([string]$Mode) {
    switch ($Mode) {
        "prebuilt" { return @("-f", "docker-compose.yml", "-f", "docker-compose.prebuilt.yml") }
        "build" { return @("-f", "docker-compose.yml", "-f", "docker-compose.build.yml") }
        default { return @("-f", "docker-compose.yml") }
    }
}

function Test-GhcrLoggedIn {
    $cfg = Join-Path $env:USERPROFILE ".docker\config.json"
    if (-not (Test-Path $cfg)) { return $false }
    return (Select-String -Path $cfg -Pattern '"ghcr.io"' -Quiet)
}

function Ensure-RegistryAuth {
    if (Test-GhcrLoggedIn) {
        Write-ClawzLog "Using existing docker login for ghcr.io"
        return
    }
    $token = $env:GITHUB_TOKEN
    if (-not $token) { $token = $env:CLAWZ_REGISTRY_TOKEN }
    if (-not $token) { $token = $env:GHCR_TOKEN }
    if (-not $token) {
        throw "GITHUB_TOKEN required for prebuilt pull (PAT with read:packages). See docs/private-registry.md"
    }
    $user = $env:GITHUB_USER
    if (-not $user) { $user = $env:CLAWZ_REGISTRY_USER }
    if (-not $user) {
        throw "Set GITHUB_USER or CLAWZ_REGISTRY_USER (GitHub username, not email)."
    }
    Write-ClawzLog "Logging in to ghcr.io as $user ..."
    $token | docker login ghcr.io -u $user --password-stdin | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "docker login ghcr.io failed"
    }
}

function Install-DockerDesktop {
    if (Get-Command docker -ErrorAction SilentlyContinue) {
        try {
            docker info 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) {
                Write-ClawzLog "Docker is already installed and running."
                return
            }
        } catch { }
    }
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-ClawzLog "Installing Docker Desktop via winget (may require a reboot) ..."
        winget install --id Docker.DockerDesktop -e --accept-source-agreements --accept-package-agreements
        Write-ClawzWarn "Start Docker Desktop from the Start menu, then re-run this installer."
        return
    }
    throw "Docker not found. Install Docker Desktop: https://docs.docker.com/desktop/setup/install/windows-install/ or run with -InstallDocker when winget is available."
}

function Invoke-MigrateDb {
    $migrate = Join-Path (Get-Location) "scripts\migrate-db.sh"
    if (-not (Test-Path $migrate)) {
        Write-ClawzWarn "scripts/migrate-db.sh not found — skipping migrations"
        return
    }
    $env:COMPOSE = "docker compose"
    $env:COMPOSE_FILE = "docker-compose.yml"
    if (Get-Command bash -ErrorAction SilentlyContinue) {
        Write-ClawzLog "Applying SQL migrations..."
        bash $migrate
        if ($LASTEXITCODE -ne 0) { throw "Database migrations failed" }
        return
    }
    if (Get-Command wsl -ErrorAction SilentlyContinue) {
        Write-ClawzLog "Applying SQL migrations via WSL..."
        wsl bash "./scripts/migrate-db.sh"
        if ($LASTEXITCODE -ne 0) { throw "Database migrations failed (WSL)" }
        return
    }
    Write-ClawzWarn "bash/WSL not found — run scripts/migrate-db.sh manually after db is up"
}

function Install-DockerStack {
    if ($InstallDocker) {
        Install-DockerDesktop
    }
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        throw "Docker not found. Install Docker Desktop or use -InstallDocker with winget."
    }
    docker info 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Docker daemon is not running. Start Docker Desktop and retry."
    }

    $useBuild = $Build.IsPresent
    if ($Prebuilt.IsPresent) { $useBuild = $false }
    if (-not $Build.IsPresent -and -not $Prebuilt.IsPresent) {
        $useBuild = $false
    }

    $composeMode = if ($useBuild) { "build" } else { "prebuilt" }
    $composeArgs = Get-ComposeArgs $composeMode

    Write-EnvFile
    $env:CLAWZ_REGISTRY = $Registry
    $env:CLAWZ_IMAGE_TAG = $Tag
    $env:CLAWZ_AGENT_IMAGE = "${Registry}/clawz-agent:${Tag}"

    if ($useBuild) {
        Write-ClawzLog "Building gateway and worker from source (first run may take 10–20 minutes)..."
        docker compose @composeArgs build gateway worker
        if ($LASTEXITCODE -ne 0) { throw "docker compose build failed" }
    } else {
        Ensure-RegistryAuth
        Write-ClawzLog "Pulling prebuilt images from $Registry ..."
        docker compose @composeArgs pull gateway worker
        if ($LASTEXITCODE -ne 0) {
            throw "Prebuilt image pull failed. Set GITHUB_TOKEN + GITHUB_USER or use -Build."
        }
    }

    Write-ClawzLog "Starting Postgres..."
    docker compose @composeArgs up -d db
    if ($LASTEXITCODE -ne 0) { throw "failed to start db" }

    Invoke-MigrateDb

    Write-ClawzLog "Starting worker and gateway..."
    docker compose @composeArgs up -d worker gateway
    if ($LASTEXITCODE -ne 0) { throw "failed to start worker/gateway" }

    Wait-Gateway
    $label = if ($useBuild) { "Docker Compose (built from source)" } else { "Docker Compose (prebuilt images)" }
    Show-Success $label
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

function Install-BootstrapOnly {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw "Git is required. Install from https://git-scm.com/download/win"
    }
    if ($InstallDocker) {
        Install-DockerDesktop
    } elseif (Get-Command docker -ErrorAction SilentlyContinue) {
        Write-ClawzLog "Docker CLI found."
    } else {
        Write-ClawzWarn "Docker not installed. Use -InstallDocker or install Docker Desktop manually."
    }
    if ($WithWeb -and -not (Get-Command npm -ErrorAction SilentlyContinue)) {
        Write-ClawzWarn "Node/npm not found — required for -WithWeb dashboard builds."
    }
    Write-ClawzLog "Bootstrap complete (host dependencies only)."
}

Write-ClawzLog "ClawZ installer — Windows"
$root = Ensure-Repo
Write-ClawzLog "Using repository at $root"

if ($BootstrapOnly) {
    Install-BootstrapOnly
    exit 0
}

$mode = if ($Source) { "source" } elseif ($Docker -or $Build -or $Prebuilt) { "docker" } else { "auto" }

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
        Write-ClawzWarn "Docker not available — falling back to source install (or use -InstallDocker)"
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

if ($Wizard) {
    Write-ClawzLog "Launching setup wizard (clawz onboard --install-daemon) ..."
    if (Get-Command clawz -ErrorAction SilentlyContinue) {
        clawz onboard --install-daemon
    } elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
        cargo run -p clawz-cli -- onboard --install-daemon
    } else {
        Write-ClawzWarn "clawz CLI not on PATH — open $GatewayUrl/setup in a browser"
    }
}
