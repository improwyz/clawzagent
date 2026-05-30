# Install & Onboarding Wizard — Implementation Task List

**Design spec:** [superpowers/specs/2026-05-30-install-onboarding-wizard-design.md](superpowers/specs/2026-05-30-install-onboarding-wizard-design.md)  
**Last updated:** 2026-05-30  

Use this file to track progress. Mark items `[x]` when done.

**Legend:** `P1` Phase 1 (MVP) · `P2` Web · `P3` AI/OAuth · `P4` Mobile/polish

---

## 0. Documentation & planning

- [x] Design spec written (`docs/superpowers/specs/2026-05-30-install-onboarding-wizard-design.md`)
- [x] Granular task list (this file)
- [x] Link wizard from `INSTALL.md` first-run section
- [x] Add architecture note to `AGENTS.md` (setup engine overview, 1 paragraph)
- [x] `.gitignore` allowlist for design spec + task tracker

---

## 1. `clawz-setup` crate foundation (P1)

### 1.1 Crate scaffold

- [ ] Add `crates/clawz-setup/` to workspace `Cargo.toml`
- [ ] Define `SetupSession`, `SetupStep`, `SetupPlatform`, `DeploymentChoice`
- [ ] Define `SetupEvent`, `SetupArtifact`, `SetupError`
- [ ] Session persistence: read/write `~/.clawz/setup/session.json`
- [ ] `setup_complete` flag in `~/.clawz/config.json`
- [ ] Unit tests: session save/load, step transitions

### 1.2 State machine

- [ ] Implement `SetupStateMachine` with phases 0–10
- [ ] Allow resume from last incomplete step
- [ ] `reset()` and `abort()` handlers
- [ ] Validate transitions (no skip secrets before deploy mode)
- [ ] Export JSON schema for web/CLI hosts

### 1.3 Host spec checker

- [ ] `HostSpecChecker::collect()` — OS, arch, RAM, disk, CPU count
- [ ] Check Docker / compose v2 availability
- [ ] Check Rust ≥ 1.87, optional Node for `--with-web`
- [ ] GHCR / `GITHUB_TOKEN` probe when prebuilt selected
- [ ] Threshold warnings per mode (standalone / micro / elastic)
- [ ] Human-readable report struct (shared with doctor)

### 1.4 Setup tools (allowlisted)

- [ ] `SetupTools` trait + registry (no arbitrary shell)
- [ ] `tool:spec_check` → HostSpecChecker
- [ ] `tool:doctor_run` → wrap existing doctor checks
- [ ] `tool:write_env` → patch `.env` with confirm token
- [ ] `tool:init_workspace` → invoke `init-workspace.sh` logic in Rust
- [ ] `tool:compose_up` / `compose_down` → install-common patterns
- [ ] `tool:migrate_db` → call migrate-db.sh or SQL embed
- [ ] `tool:create_agent` → build `AgentConfig` payload
- [ ] `tool:set_provider` → provider config DTO for gateway
- [ ] `tool:smoke_execute` → test agent turn
- [ ] Confirm-gate wrapper for destructive tools

### 1.5 Deploy planner

- [ ] `DeployPlanner`: prebuilt vs build decision
- [ ] Map to `CLAWZ_IMAGE_TAG`, compose file overlays
- [ ] Source-run path for `CLAWZ_MODE=standalone` without Docker
- [ ] Return actionable errors (GHCR auth, port in use)

### 1.6 Agent bootstrap

- [ ] `AgentBootstrap::build_identity` — name, who_am_i, role → `AGENTS.md` + system_prompt
- [ ] Skill selection → workspace `skills/*/SKILL.md`
- [ ] Standalone: single `AgentConfig`
- [ ] Call gateway client or local API to `POST /agents`
- [ ] Wire `WorkspaceLoader` path (`CLAWZ_WORKSPACE` / `~/.clawz/workspace`)

---

## 2. CLI & doctor integration (P1)

### 2.1 Shared doctor

- [ ] Move/duplicate doctor checks into `clawz-setup::doctor`
- [ ] `clawz doctor` calls shared module (thin wrapper in CLI)
- [ ] JSON output includes remediation hints
- [ ] `doctor --fix` stub (lists suggested tools only, P3 executes)

### 2.2 `clawz onboard` rewrite

- [ ] `onboard.rs` drives `SetupStateMachine` instead of print-only TUI
- [ ] Flags: `--resume`, `--step <n>`, `--json`, `--non-interactive` (scripted)
- [ ] `--install-daemon` → compose up after phase 3
- [ ] On success: print dashboard URL + `setup_complete`

### 2.3 Dependency installer

- [ ] `DependencyInstaller` wraps `install-deps.sh` with dry-run
- [ ] Sudo consent callback (TUI prompt / JSON confirm id)
- [ ] Idempotent: skip already-satisfied deps
- [ ] Log output captured for AI context (last N lines)

---

## 3. Linux TUI (P1)

### 3.1 Ratatui wizard UI

- [ ] Add `ratatui` + `crossterm` to `clawz-tui`
- [ ] Multi-screen flow: welcome → mode → deploy → … → complete
- [ ] Progress bar / step indicator (0–10)
- [ ] Summary screen before apply
- [ ] Stdin fallback when `CLAWZ_TUI=plain` or non-TTY

### 3.2 TUI screens per phase

- [ ] Phase 0–1: spec summary + mode picker
- [ ] Phase 2: prebuilt vs build + GHCR token prompt
- [ ] Phase 4: show generated secrets (masked) + confirm write `.env`
- [ ] Phase 5: LLM provider + API key (manual path P1)
- [ ] Phase 6–7: identity + skills (text inputs)
- [ ] Phase 8: topology (standalone auto / micro template stub)
- [ ] Phase 9: doctor results inline (green/red)
- [ ] Phase 10: completion + next steps

### 3.3 TUI tests

- [ ] Snapshot or integration test with `CLAWZ_TUI=plain` + piped answers
- [ ] Mask secrets in test output

---

## 4. Install script fallback (P1)

- [ ] `install.sh --wizard` flag
- [ ] After clone/deps: exec `clawz onboard --json` or cargo run CLI
- [ ] Pass `--install-daemon` when docker mode
- [ ] Document in `install.sh` header comment
- [ ] `install.ps1 --wizard` stub (message: use web on Windows until P4)

---

## 5. Gateway setup API (P2)

### 5.1 Routes & auth

- [ ] New module `crates/clawz-gateway/src/routes/setup.rs`
- [ ] Bootstrap token middleware (env `CLAWZ_SETUP_BOOTSTRAP_TOKEN`, one-time file)
- [ ] `GET /api/v1/setup/status`
- [ ] `POST /api/v1/setup/session` (start/resume)
- [ ] `POST /api/v1/setup/answer`
- [ ] `POST /api/v1/setup/apply`
- [ ] `POST /api/v1/setup/complete`
- [ ] Disable bootstrap routes when `setup_complete`

### 5.2 Server-side apply

- [ ] Apply `.env` recommendations to `UiSettings` / provider store
- [ ] Persist `setup_complete` in gateway state + disk
- [ ] PRISM-G audit entries for setup mutations
- [ ] Integration test: bootstrap token → complete flow (mocked docker)

### 5.3 OAuth callbacks (P2/P3)

- [ ] `POST /api/v1/setup/oauth/start` (provider enum)
- [ ] `POST /api/v1/setup/oauth/callback`
- [ ] Cursor OAuth flow
- [ ] Anthropic OAuth / API key
- [ ] OpenAI OAuth / API key
- [ ] Codex token exchange (document env vars)
- [ ] Store tokens in setup-only vault (not gateway providers until promoted)

---

## 6. Web wizard — Win/Mac primary (P2)

### 6.1 Routing & guard

- [ ] `web/src/pages/Setup.tsx` — stepper UI matching phases 0–10
- [ ] `web/src/lib/setup.ts` — API client for setup routes
- [ ] Router guard: redirect to `/setup` if `!setup_complete`
- [ ] `ShellBootstrap` waits for setup status before dashboard

### 6.2 UI components

- [ ] Welcome + spec cards (RAM, Docker, disk)
- [ ] Deploy mode cards (standalone / micro / elastic)
- [ ] Prebuilt vs build selector + GHCR instructions
- [ ] Secrets review panel (masked)
- [ ] LLM provider picker + key input
- [ ] Identity form: name, who am I, role, tone
- [ ] Skills checklist (workspace skills)
- [ ] Topology picker (micro/elastic)
- [ ] Doctor results panel with retry
- [ ] Completion screen + link to dashboard

### 6.3 Win/Mac specifics

- [ ] Detect platform; show “remote gateway URL” path (no local Docker)
- [ ] WSL2 guidance link for Windows Docker
- [ ] Keychain note for desktop (future Tauri bridge)

### 6.4 Web tests

- [ ] Playwright or vitest: setup redirect when incomplete
- [ ] Mock setup API responses

---

## 7. OAuth consent & MSP path (P3)

- [ ] OAuth provider selection screen (all hosts)
- [ ] “Skip OAuth — configuring for a customer” → manual deterministic mode
- [ ] End-of-wizard consent: setup-only | promote to runtime | discard
- [ ] Promote: write gateway provider + optional `.env` for Docker
- [ ] Setup-only: keyring namespace `clawz-setup`; purge on complete if selected
- [ ] MSP copy: customer adds LLM keys in Config page (link in wizard)
- [ ] Never log raw tokens; audit promotion events

---

## 8. AI-first setup agent (P3)

### 8.1 Setup agent runner

- [ ] `SetupAgentRunner` sidecar: CLI subprocess `clawz setup-agent` (Linux)
- [ ] System prompt contract (allowlisted tools, phases, confirm rules)
- [ ] Cursor SDK integration (`Agent.create`, local cwd, explicit `local`)
- [ ] Stream events → host (TUI/web SSE)
- [ ] Inject inline context: spec + doctor JSON + last 50 log lines

### 8.2 Tool calling from AI

- [ ] Map model tool calls → `SetupTools` registry
- [ ] Reject unknown tools
- [ ] Confirm tokens for destructive ops returned to host UI
- [ ] Max iterations / timeout per phase

### 8.3 Codex rescue path

- [ ] On doctor failure after 1 retry: offer “Fix with Codex”
- [ ] Single `task` invocation with full doctor output inline
- [ ] User confirm before `--write` fixes
- [ ] Re-run doctor after fix (max 3 loops)

### 8.4 Provider-specific AI auth

- [ ] Cursor: API key + OAuth
- [ ] Codex: token env
- [ ] Anthropic: API key + OAuth (wizard only until promoted)
- [ ] OpenAI: API key + OAuth (wizard only until promoted)
- [ ] Fallback to deterministic steps if AI unavailable

---

## 9. Multi-agent templates (P3)

- [ ] Template: single ops agent
- [ ] Template: orchestrator + worker (2 agents, team metadata)
- [ ] Template: custom N (form + validation)
- [ ] Budget fields in metadata (daily limit optional)
- [ ] Gateway batch create + rollback on partial failure
- [ ] TUI + web template picker UI

---

## 10. Mobile & polish (P4)

- [ ] `clawz-tauri-mobile`: first launch → `/setup` if incomplete
- [ ] Deep link / QR “continue setup on desktop” for long deploy steps
- [ ] Resumable session sync (optional): export session token
- [ ] `install.ps1 --wizard` full parity on Windows
- [ ] ratatui accessibility: high-contrast theme
- [ ] i18n stub (English only v1; string table placeholder)

---

## 11. Verification & release

- [ ] E2E: Linux headless `clawz onboard --json` + piped answers → doctor green
- [ ] E2E: docker compose micro path (CI compose-smoke + wizard)
- [ ] E2E: web setup flow against test gateway
- [ ] Security review: bootstrap token, secret handling, audit chain
- [ ] Update `docs/deployment-build-strategy.md` § first-run
- [ ] CHANGELOG entry

---

## Progress summary

| Phase | Total tasks | Done |
|-------|-------------|------|
| 0. Docs | 5 | 5 |
| 1. clawz-setup | 35 | 0 |
| 2. CLI/doctor | 12 | 0 |
| 3. Linux TUI | 14 | 0 |
| 4. install.sh | 5 | 0 |
| 5. Gateway API | 18 | 0 |
| 6. Web wizard | 17 | 0 |
| 7. OAuth/MSP | 7 | 0 |
| 8. AI agent | 16 | 0 |
| 9. Multi-agent | 6 | 0 |
| 10. Mobile | 6 | 0 |
| 11. Verification | 6 | 0 |
| **Approx. total** | **~147** | **5** |

_Update the table counts when checking off items._
