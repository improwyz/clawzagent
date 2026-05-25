# ClawZ Agent Orchestration Platform — Security Policy

**Version:** 1.0  
**Date:** 2026-05-24  
**License:** Elastic License 2.0 (ELv2)  
**Maintainer:** Enterpryz Ventures  

---

## Executive Summary

ClawZ is a multi-tenant agent orchestration platform that orchestrates autonomous AI agents across distributed infrastructure. This security policy defines the trust model, threat assumptions, vulnerability handling, and defense mechanisms that protect tenant data, ensure isolation, and maintain platform integrity.

ClawZ is designed with **defense in depth**: every layer assumes the layers below it may be compromised and implements independent security controls.

---

## Trust Model

### Platform Components

ClawZ operates three distinct trust zones:

#### 1. Untrusted Zone (External API Callers)
- **Definition:** Any external system interacting with ClawZ gateway endpoints
- **Examples:** Third-party API clients, webhook receivers, external LLM providers
- **Assumptions:**
  - All input is malicious until validated
  - All external parties may attempt privilege escalation, tenant isolation bypass, or audit tampering
  - Network packets may be inspected or modified in transit
- **Controls:** API key authentication, input validation, rate limiting, audit logging

#### 2. Sandboxed Zone (Agent Runtime)
- **Definition:** Agent containers and in-process sandboxes executing user-defined logic
- **Examples:** Docker containers running agents, standalone mode Rust trait boundaries
- **Assumptions:**
  - **Agents are potentially compromised.** A malicious agent may attempt to:
    - Exfiltrate data from other agents' namespaces
    - Modify audit logs
    - Escape sandbox boundaries
    - Consume excessive resources to cause DoS
    - Tamper with governance decisions
  - External API calls from agents may be redirected or intercepted
- **Controls:** Container isolation (Docker or trait boundaries), resource limits, governance enforcement, audit chain validation

#### 3. Trusted Zone (Platform Internals)
- **Definition:** Core platform services with full system access
- **Examples:** Gateway, WorkerPool, runtime orchestrator, audit service
- **Assumptions:**
  - Platform code is reviewed and not intentionally malicious
  - Database connections are protected
  - Credential stores are not world-readable
- **Controls:** Code review, principle of least privilege, encrypted credential storage

### Tenant Isolation

- Each tenant operates within a **tenant scope** isolation boundary
- Operators (authenticated API callers) are trusted **only within their assigned tenant and RBAC role**
- An operator with access to Tenant A must **not** be able to:
  - Read agent definitions, execution logs, or cost data from Tenant B
  - Modify Tenant B's governance policies or budgets
  - Impersonate agents in Tenant B
  - Access audit trails for Tenant B actions

---

## Reporting Vulnerabilities

We take security seriously and are committed to working with security researchers transparently.

### Disclosure Policy

**DO NOT** open public GitHub issues for security bugs. Private disclosure protects users before patches are available.

#### Reporting Channels

1. **GitHub Security Advisories** (Preferred)
   - Visit: https://github.com/enterpryz-ventures/clawz-agent/security/advisories
   - Create private security advisory
   - ClawZ maintainers will be automatically notified

2. **Email** (Alternative)
   - Send to: `security@enterpryz.io`
   - Subject line: `[SECURITY] ClawZ Agent Vulnerability Report`
   - PGP key available at: https://enterpryz.io/security/pgp.asc

### Response Timeline

- **48 hours:** Acknowledgment of receipt and initial assessment
- **7 days:** Detailed impact assessment and proposed fix timeline
- **Critical vulnerabilities:** Patch released within 2 weeks
- **High/Medium:** Patch released within 30 days
- **Low:** Included in next scheduled release (60+ days)

### Vulnerability Report Contents

Please include:

1. **Description:** Clear summary of the vulnerability
2. **Attack Vector:** How an attacker would exploit this (e.g., "unauthenticated network request")
3. **Affected Component:** Which ClawZ module (e.g., "gateway", "audit service", "container isolation")
4. **Reproduction Steps:** Minimal, step-by-step instructions to trigger the bug
5. **Impact Assessment:** 
   - Which trust zone(s) are affected
   - Tenant isolation bypass? Yes/No
   - Audit trail tampering? Yes/No
   - Credential exposure? Yes/No
6. **Suggested Fix:** If you have a remediation in mind
7. **CVE Information:** If this is a known CVE, include the identifier

### Public Disclosure

After a patch is released and deployed (typically 72 hours), ClawZ will:
- Publish a security advisory on GitHub
- Credit the reporter (with permission)
- Recommend upgrade urgently

---

## What Qualifies as a Security Bug

### Confirmed Security Issues

The following issues are confirmed security bugs and should be reported immediately:

1. **Tenant Isolation Bypass**
   - One tenant can access, modify, or observe another tenant's data
   - Agent in Tenant A can read environment variables from Tenant B's agents
   - Governance policies from Tenant A affect Tenant B's agents

2. **Authentication Bypass**
   - API request succeeds without valid API key
   - API key from one tenant grants access to another tenant's resources
   - JWT or session token validation is skipped

3. **Privilege Escalation**
   - Operator exceeds RBAC role (e.g., reader role can write)
   - Operator in Tenant A gains access to Tenant B's resources
   - Agent gains access to platform internals (Trusted zone)

4. **Governance Bypass** (PRISM-G Evasion)
   - Agent action skips Privacy, Reliability, Integrity, Safety, Monitoring, or Governance checks
   - Malicious policy is not rejected by policy evaluator
   - Council consensus is bypassed for approval workflows
   - Approval workflows execute without required signatures

5. **Audit Chain Tampering**
   - Audit entry is modified after creation
   - Hash chain verification fails silently instead of raising error
   - Audit log entry is deleted or truncated
   - Timestamps are inconsistent (backwards in time)

6. **Container Escape**
   - Agent container gains access to host filesystem
   - Agent container can access other containers' mounts
   - Agent process runs as root despite non-root requirement
   - cgroup resource limits are not enforced

7. **Credential Exposure**
   - API keys, database passwords, or provider credentials appear in:
     - HTTP response bodies
     - Log files (stdout/stderr)
     - Error messages returned to clients
     - Unencrypted telemetry

8. **Injection Attacks**
   - SQL injection in database queries
   - Command injection in container execution
   - Path traversal in file access
   - Unauthorized variable substitution in agent environment

---

## What Usually Is NOT a Security Bug

The following reports typically do not constitute security vulnerabilities:

1. **Prompt Injection**
   - An agent receives user input that changes its behavior
   - Agents operate **within their governance constraints** — PRISM-G enforces guardrails even if the prompt tries to bypass them
   - This is a model behavior issue, not a ClawZ platform vulnerability
   - Report to the LLM provider, not ClawZ

2. **Trusted Operator Misuse**
   - An operator with write access uses that access intentionally
   - A tenant operator with budget allocation modifies their own budget
   - A platform admin makes intentional configuration changes
   - These are policy violations, not bugs

3. **Resource Exhaustion by Authenticated User**
   - An operator runs agents within their allocated budget and observes high costs
   - An agent consumes CPU/memory up to its configured limits
   - Authentication is required; the user is entitled to their resources
   - This is expected behavior, not a vulnerability

4. **Scanner-Only Reports**
   - A security scanner reports a CVE affecting a dependency
   - No demonstrated attack path in ClawZ exists
   - Report to maintainers only after proving exploitability

5. **Information Disclosure Within Tenant**
   - An operator can observe the agent logs they created
   - An operator can see the budget they spent
   - Operators in the same tenant see each other's agents
   - This is expected multi-operator behavior within a tenant scope

6. **Policy Design Disagreements**
   - ClawZ enforces a governance policy differently than expected
   - An approval workflow requires multiple signatures for certain actions
   - Budget limits apply per-agent instead of per-tenant
   - These are feature requests, not security bugs

---

## Security Architecture

ClawZ implements multiple independent defense layers. Compromise of one layer does not compromise others.

### Layer 1: Runtime Isolation

#### Container Mode (Elastic Deployments)

Agents run in isolated Docker containers via the **Bollard scheduler**:

- **Non-root execution:** Agents run as unprivileged UID (configurable, default: `1000`)
- **Filesystem isolation:** Each agent has its own `/tmp`, `/var` mount
- **Resource limits:**
  - CPU: configurable per-agent (e.g., 0.5 cores)
  - Memory: configurable per-agent (e.g., 512 MB)
  - Disk: tmpfs mount size limited
- **Network isolation:** Each container has its own network namespace; firewall rules applied per-tenant
- **Process isolation:** Container cannot see host processes or other containers' processes
- **Capability dropping:** Unnecessary Linux capabilities removed (`CAP_NET_ADMIN`, etc.)

**Configuration:** See `crates/clawz-worker/src/orchestration/bollard_scheduler.rs`

#### In-Process Mode (Standalone Deployments)

Agents run within the same process but with trait-based isolation:

- **Trait boundaries:** Agent code implements `AgentRuntime` trait with explicit capabilities
- **Memory sandbox:** No direct memory access to other agents' state
- **Panic isolation:** Agent panic is caught; execution continues
- **Type system:** Rust type system prevents illegal access at compile time

**Configuration:** See `crates/clawz-worker/src/runtime/agent.rs`

**Trade-off:** Standalone mode trades perfect isolation for deployment simplicity. Use only for single-tenant deployments or trusted agent code.

#### Elastic Mesh Mode

Multi-tenant mesh deployment with network-level isolation:

- **Per-tenant firewall rules:** Network policies restrict inter-tenant traffic
- **Sidecar proxies:** Each agent pod includes a sidecar that enforces network ACLs
- **mTLS:** Agent-to-agent communication encrypted and authenticated
- **Service mesh:** Istio or Cilium integration for traffic control

**Configuration:** See `crates/clawz-worker/src/mesh/config.rs`

---

### Layer 2: Authentication & Authorization

#### API Key Authentication

ClawZ gateway authenticates all incoming API requests using cryptographic API keys:

- **Configuration:** `VALID_API_KEYS` environment variable (format: `key1:sha256_hash1,key2:sha256_hash2`)
- **Scope:** Each API key is bound to a single tenant
- **Rotation:** Keys should be rotated quarterly; old keys revoked immediately upon compromise
- **Hashing:** API keys are hashed with SHA-256; plaintext keys never stored
- **Logging:** Failed auth attempts logged (without leaking key material) with IP, timestamp, tenant scope

#### Cascade RBAC

Authorization follows a **cascade hierarchy**:

```
Organization
  ├── Tenant A (API key: tenant-a-key)
  │   ├── Agent 1 (Agent reads, observes logs)
  │   ├── Agent 2 (Agent reads only)
  │   └── [budget allocation: $10/day]
  ├── Tenant B (API key: tenant-b-key)
  │   ├── Agent 3 (Agent writes, creates sub-agents)
  │   └── [budget allocation: $5/day]
```

**Roles:**

- **Reader:** View agent definitions, logs, audit trails (own tenant only)
- **Writer:** Create, modify, delete agents; update policies (own tenant only)
- **Admin:** Full tenant access including budget reallocation, key rotation
- **Platform Admin:** (Rare) Access to all tenants, system configuration, governance policies

**Implementation:** See `crates/clawz-core/src/types/agent.rs`

#### Budget Sub-Leasing

Tenants can allocate budgets to child agents:

```
Tenant A Budget: $100/day
  ├── Agent 1: $30/day
  ├── Agent 2: $40/day
  └── Agent 3: $30/day
```

**Enforcement:**
- Agent execution stops when daily budget exceeded
- Monthly rollover resets counters
- Budget overages logged to audit trail
- Parent tenant responsible for cost if children exceed allocation

**Implementation:** See `crates/clawz-core/src/types/cost.rs`

---

### Layer 3: Governance (PRISM-G Framework)

Every agent action is evaluated against **six governance dimensions**:

#### Privacy (P)
- **PII Detection:** Automatic detection of names, email, phone, SSN, medical IDs
- **Redaction:** Matched PII is redacted in logs, audit trails, and responses
- **Data Residency:** Agent execution zone matches data residency policy
- **Encryption:** Data at-rest and in-transit encrypted with tenant-specific keys

#### Reliability (R)
- **Provider Health:** Circuit breakers detect failing LLM/API providers
- **Retry Policies:** Exponential backoff with jitter for transient failures
- **Timeout Enforcement:** Hard timeout on agent execution (configurable, default: 5 min)
- **Fallback:** If primary provider fails, switch to backup (if configured)

#### Integrity (I)
- **Output Validation:** Agent output schema validated before persistence
- **Hash Chain Audit:** Each audit entry includes SHA-256 hash of previous entry
- **Tamper Detection:** Audit verification detects any modification
- **Determinism:** Agent outputs logged with input hash for replay verification

#### Safety (S)
- **Content Filtering:** Hate speech, NSFW, dangerous instructions detected and rejected
- **Action Approval:** High-risk actions (delete, modify budget) require explicit approval
- **Rate Limiting:** Agent cannot execute >N times per minute
- **Resource Quotas:** Agent cannot allocate >N new sub-agents per hour

#### Monitoring (M)
- **Telemetry:** All actions emitted as OpenTelemetry spans
- **Prometheus Metrics:** `clawz_agent_executions_total`, `clawz_audit_entries_total`, etc.
- **Log Aggregation:** Structured JSON logs compatible with ELK, Datadog, CloudWatch
- **Alerting:** Anomaly detection alerts on unusual tenant behavior (bulk execution, policy violations)

#### Governance (G)
- **Policy Evaluation:** Every action evaluated against tenant's governance.yaml policy
- **Trust Scoring:** Agent behavior tracked; low-trust agents face stricter controls
- **Council Consensus:** High-risk actions require approval from governance council (N-of-M signatures)
- **Immutable Audit:** Governance decisions logged to immutable audit chain

**Configuration:** See `crates/clawz-worker/src/governance/` module

**CRITICAL:** PRISM-G is **not optional** in production. Disabling any dimension creates security risk.

---

### Layer 4: Audit Trail & Compliance

ClawZ maintains a **cryptographically-signed audit trail** for compliance and forensics:

#### SHA-256 Hash Chain

Each audit entry contains:

```json
{
  "id": "audit-12345",
  "timestamp": "2026-05-24T10:30:00Z",
  "actor": "api-key:tenant-a",
  "action": "agent.execute",
  "resource": "agent:agent-1",
  "result": "success",
  "prism_g_decision": "approved",
  "data": { "cost": 0.15 },
  "previous_hash": "sha256:abc123...",
  "current_hash": "sha256:def456..."
}
```

**Previous Hash Verification:**
- Each entry cryptographically links to the previous entry
- Hash mismatch indicates tampering
- Verification runs on read from database
- Tampered entries logged as security incidents

#### Compliance Evidence Export

ClawZ can export audit trails in compliance formats:

- **SOC 2 Type II:** 12-month evidence of access controls, change logging, incident response
- **GDPR:** Data subject access requests, consent logs, deletion evidence
- **EU AI Act:** Risk assessment, deployment configurations, governance decisions
- **HIPAA:** If agents process PHI, encryption and access logs verified

**Generation:** Use admin CLI: `clawz audit export --format=soc2 --period=12m`

---

### Layer 5: Credential Management

**Critical Rule:** No credentials in code, config files, or logs.

#### API Key Loading

- **Source:** `VALID_API_KEYS` environment variable only
- **Format:** `key1:hash1,key2:hash2` (hashed, never plaintext)
- **Rotation:** Restart required; old keys revoked
- **Emergency Revocation:** Remove from env var and restart (immediate effect)

#### Provider Credentials

- **Source:** `ProviderConfig` struct loaded from database or environment
- **Encryption:** Encrypted at-rest using tenant-specific key
- **Access:** Only provider runtime can access; platform code receives opaque token
- **Logging:** Provider credentials never logged, even in DEBUG mode
- **Example:** OpenAI API key stored encrypted; only provider scheduler sees plaintext

#### Database Credentials

- **Source:** `DATABASE_URL` environment variable only
- **Format:** `postgresql://user:pass@host/db` (SSL required in production)
- **Rotation:** Change in environment and restart
- **Least Privilege:** Database user has only necessary schema permissions (INSERT, SELECT on audit_logs; no DROP)

#### TOML Configuration Files

- **Rule:** NEVER store credentials in Cargo.toml, config.toml, or other config files
- **Violation:** Security audit finding; immediate patch required
- **Exception:** Localhost development only (never commit; add to .gitignore)

**Implementation:** See `crates/clawz-core/src/config.rs`

---

### Layer 6: Network Security

#### Transport Protocols

ClawZ supports multiple secure transports:

1. **gRPC over TLS**
   - Default: Agent-to-gateway communication
   - Certificate pinning supported
   - mTLS for agent-to-agent in mesh mode

2. **QUIC (0-RTT)**
   - Low-latency for latency-sensitive agents
   - Forward secrecy maintained (no key reuse)
   - 0-RTT disabled in production (uses previous session key)

3. **WebSocket Secure (WSS)**
   - Browser-based agent control panels
   - JWT token authentication in WebSocket upgrade
   - Cross-origin requests restricted via CORS policy

#### Firewall Rules

**Container Mode:**
- Ingress: Only gateway can connect to agents (port 50051 internal)
- Egress: Agents can connect to configured providers (OpenAI, Anthropic, etc.) via outbound firewall rules
- No ingress from other agents or external clients

**Mesh Mode:**
- Per-tenant namespace isolation (Kubernetes network policy)
- Sidecar proxies enforce ACLs
- Cross-tenant traffic explicitly denied

#### Reverse Proxy (Recommended Production Setup)

ClawZ gateway should **never** be exposed directly to the internet. Use a reverse proxy:

```
Internet → TLS Terminator (Nginx/HAProxy)
         → Rate Limiter (Cloudflare/AWS WAF)
         → ClawZ Gateway (internal network)
```

**Proxy Configuration:**
- TLS 1.3 only
- Strong cipher suites only
- Rate limiting: 100 req/sec per API key
- DDoS protection enabled
- Request logging with audit trail integration

---

### Layer 7: Cost Controls & Resource Limits

ClawZ enforces multi-level cost controls to prevent accidental or malicious overspend:

#### Cost Tracking

- **Per-Request Tracking:** LLM cost (tokens × rate) tracked in `CostRepo`
- **Per-Agent Accumulation:** Daily and monthly totals computed
- **Budget Enforcement:** Execution blocked when:
  - Daily budget exceeded
  - Monthly budget exceeded
  - Estimated request cost would exceed remaining budget

#### Resource Limits

Each agent has configurable limits:

```
Agent Config:
  cpu_limit: 0.5            # CPU cores
  memory_limit: 512         # MB RAM
  disk_limit: 1024          # MB tmpfs
  timeout: 300              # seconds
  daily_budget: 10.0        # USD
  monthly_budget: 200.0     # USD
  max_sub_agents: 5         # children limit
  rate_limit: 10            # executions/min
```

**Enforcement Points:**
1. Container scheduler enforces CPU/memory (Linux cgroups)
2. Agent runtime enforces timeout (async cancellation)
3. Cost service enforces budget (pre-execution check)
4. Governance engine enforces policy limits (rate limiting, sub-agent creation)

---

## Container Security Hardening

### Base Image Selection

ClawZ provides minimal Docker images:

- **Dockerfile.gateway:** Alpine Linux + Rust binary (minimal attack surface)
- **Dockerfile.worker:** Alpine Linux + agent scheduler (minimal dependencies)
- **Size:** <100 MB each (verifiable: `docker image inspect --format='{{.Size}}'`)

### Non-Root Execution

**Requirement:** All containers run as non-root UID

```dockerfile
RUN useradd -u 1000 -m clawz
USER 1000:1000
```

**Enforcement:** Kubernetes security policy or Docker run-time check

### Read-Only Filesystem

Agents can run with read-only root filesystem:

```dockerfile
RUN --mount=type=tmpfs,target=/tmp \
    --mount=type=tmpfs,target=/var/tmp
```

**Benefits:**
- Prevents agent from modifying system files
- Limits persistence of compromised agent state
- Enforces stateless design

### Resource Limits (cgroups)

CPU and memory limits enforced by Bollard scheduler:

```rust
// crates/clawz-worker/src/orchestration/bollard_scheduler.rs
HostConfig {
    cpu_shares: cpu_limit * 1024,
    memory: memory_limit * 1_000_000,
    ..
}
```

**Verification:**
```bash
docker stats --no-stream | grep agent-12345
```

---

## Dependency Security

### Rust Memory Safety

ClawZ is written in **Rust**, providing compile-time guarantees:

- **No Buffer Overflows:** Rust's bounds checking prevents out-of-bounds access
- **No Use-After-Free:** Ownership system prevents dangling pointers
- **No Data Races:** Borrow checker prevents concurrent mutation
- **No Null Pointer Dereference:** Optional types (`Option`, `Result`) force explicit handling

These guarantees hold for ClawZ code. Vulnerabilities in **dependencies** still possible.

### Dependency Auditing

**Requirement:** Weekly audit of all dependencies

```bash
cargo audit --deny warnings
# Block compilation if vulnerable deps detected
```

**Procedure:**
1. Run `cargo audit` in CI/CD pipeline
2. High/Critical: Patch immediately, release hotfix
3. Medium: Patch in next scheduled release
4. Low: Patch in next major release or defer

### Lock File Commitment

**Requirement:** `Cargo.lock` committed to version control

**Rationale:**
- Reproducible builds: Same code + lock file = identical binaries
- Supply chain security: Hash lock file in build artifacts
- Forensics: Audit which dependencies were in production at time of incident

**Verification:** Every release includes `Cargo.lock` hash in release notes

---

## Deployment Security Checklist

### Pre-Deployment

- [ ] All dependencies pass `cargo audit`
- [ ] Code reviewed for hardcoded secrets (grep `"password"`, `"key"`, `"token"`)
- [ ] `SECURITY.md` reviewed and up to date
- [ ] Threat model assessed for new features
- [ ] TLS certificates provisioned (TLS 1.3, strong ciphers)

### At Deployment

- [ ] API keys generated and stored in key management system (AWS Secrets Manager, Vault)
- [ ] Database credentials set via environment variable (no plaintext config)
- [ ] PRISM-G governance policies enabled for all tenants
- [ ] Reverse proxy configured and tested (rate limiting, TLS termination)
- [ ] Audit logging enabled and tested (verify entries written to database)
- [ ] Monitoring configured (Prometheus, Datadog, or equivalent)
- [ ] Alerting rules set for anomalies (bulk execution, policy violations, cost spikes)
- [ ] Incident response team notified and trained

### Post-Deployment

- [ ] API key rotation schedule established (quarterly minimum)
- [ ] Audit logs reviewed daily for first week
- [ ] Cost reports reviewed weekly
- [ ] Governance decisions audited monthly
- [ ] Security patches monitored (cargo audit, GitHub Dependabot)
- [ ] Annual penetration test scheduled

---

## Incident Response

### Security Incident Definition

A security incident is:
- Confirmed tenant isolation bypass
- Confirmed authentication bypass
- Confirmed credential exposure
- Audit chain tampering detected
- Container escape detected
- DoS attack causing sustained unavailability

### Response Procedure

1. **Immediate (0-1 hour):**
   - Isolate affected components (kill compromised agent containers)
   - Enable debug logging
   - Preserve forensic evidence (container logs, audit trail)
   - Notify security team

2. **Short-term (1-24 hours):**
   - Assess impact (which tenants affected, what data exposed)
   - Implement temporary mitigation (disable feature, enforce stricter policy)
   - Develop fix and test in staging

3. **Medium-term (1-7 days):**
   - Deploy fix to production
   - Verify forensic evidence (audit chain intact, no data loss)
   - Notify affected tenants with timeline and remediation steps

4. **Long-term (7-30 days):**
   - Post-mortem analysis
   - Update security controls to prevent recurrence
   - Publish security advisory (if not security.txt embargo)
   - Train team on lessons learned

---

## References & Further Reading

- [OWASP Top 10 for 2024](https://owasp.org/www-project-top-ten/)
- [NIST Cybersecurity Framework](https://www.nist.gov/cyberframework)
- [Cloud Native Security Whitepaper](https://www.cisecurity.org/)
- [Kubernetes Security Best Practices](https://kubernetes.io/docs/concepts/security/)
- [PRISM-G Governance Framework](./CLAWZ.md#prism-g)

---

## Security Policy Version History

| Version | Date       | Changes |
|---------|------------|---------|
| 1.0     | 2026-05-24 | Initial policy release |

---

**Last Updated:** 2026-05-24  
**Next Review:** 2026-11-24 (6 months)

For questions or clarifications, contact: security@enterpryz.io
