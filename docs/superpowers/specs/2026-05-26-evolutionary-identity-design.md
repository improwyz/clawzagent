# Evolutionary Identity — Design Specification

> **Generated:** 2026-05-26
> **Status:** Approved — pending implementation plan

---

## Goal

Extend `AgentIdentity` (currently: trust relationships, accumulated experience, skill proficiencies, session count) into a full **layered identity system** with:

1. **Fixed Core** — injected at startup, never mutable post-initialization, provides stable identity anchor
2. **Evolving State** — persisted across sessions, modified by runtime experience, allows the agent to grow
3. **Identity Version Hash** — canonical fingerprint of the fixed core at initialization

This enables: (a) emulation of specific human roles via injected personality seeds, and (b) authentic personality evolution through accumulated experience.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                      AgentIdentity                           │
│                                                              │
│  ┌────────────────────────┐    ┌─────────────────────────┐   │
│  │     IdentityCore       │    │     IdentityState       │   │
│  │     (immutable)        │    │     (evolvable)         │   │
│  │                        │    │                         │   │
│  │  mbti: MBTIType        │    │  behaviour: BehaviourMap│  │
│  │  temperament: Temp     │    │  self_esteem: f32       │  │
│  │  risk_posture: Risk    │    │  interests: InterestMap  │  │
│  │  processing_style: Proc│    │  response_calibration:  │   │
│  │  authority_orientation│    │    ResponseCalibration   │  │
│  │  values: Values        │    │  temporal_preference:   │   │
│  │  identity_version: u64 │    │    TemporalPreference   │  │
│  └────────────────────────┘    └─────────────────────────┘   │
│                                                              │
│  identity_version_hash: String (SHA-256 of fixed core)       │
└──────────────────────────────────────────────────────────────┘
```

### Fixed Core (`IdentityCore`)

| Field | Type | Description |
|-------|------|-------------|
| `mbti` | `MBTIType` | 4-letter type injected at startup (INTJ, ENFP, etc.); can drift in label but seed is fixed |
| `temperament` | `Temperament` | `reactivity: f32`, `self_regulation: f32` — architectural baseline, not learned |
| `risk_posture` | `RiskPosture` | `risk_tolerance: f32` — fixed threshold for acceptable risk in recommendations/actions |
| `processing_style` | `ProcessingStyle` | `parallel vs. sequential`, `reflexive vs. deliberative` — fundamental cognitive mode |
| `authority_orientation` | `AuthorityOrientation` | `deferential / skeptical / egalitarian` — core stance toward human oversight |
| `values` | `Values` | `cardinal_rule: String` (immutable "no harm to humanity"), `value_hierarchy: HashMap<String, f32>` (priorities can drift) |
| `identity_version` | `u64` | Monotonically incrementing version for each fixed-core initialization |

### Evolving State (`IdentityState`)

| Field | Type | Description |
|-------|------|-------------|
| `behaviour` | `BehaviourMap` | `HashMap<BehaviourType, f32>` — cooperative, assertive, passive, etc. Learnable patterns per context |
| `self_esteem` | `f32` | `[0, 1]` — evolves based on accumulated success/failure experience; not self-concept |
| `interests` | `InterestMap` | `HashMap<String, f32>` — domain interests that grow through exposure |
| `talent` | `TalentMap` | `HashMap<String, f32>` — skill proficiency (already tracked; migrate here) |
| `response_calibration` | `ResponseCalibration` | `directness: f32`, `assertiveness: f32`, `emotional_colour: f32` — tunable within core posture |
| `temporal_preference` | `TemporalPreference` | `horizon_baseline: Horizon` (SHORT/MEDIUM/LONG, fixed), `horizon_refinement: f32` (evolvable calibration) |

### Identity Version Hash

`identity_version_hash: String` — computed once at initialization as:

```
SHA-256(
  mbti.0 | temperament.0 | temperament.1 |
  risk_posture | processing_style |
  authority_orientation | cardinal_rule | identity_version
)
```

Gives operators a canonical, auditable fingerprint of which fixed core configuration is running.

---

## MBTI Type — Drift Rule

The MBTI seed is fixed at injection. However, the **expressive label** can drift based on observed behaviour:

- The agent's accumulated decisions and preference patterns may increasingly resemble a *different* MBTI type
- After N sessions (configurable, default 50), if the agent's observed patterns strongly match a different type (e.g., 80% of decisions align with INTJ despite being seeded as INFP), the `mbti_drift_label` field updates
- The **seed** remains unchanged (`original_mbti: MBTIType` stored separately); only the descriptive label drifts
- Operators can query: "is this agent still behaving as its seeded type?" via `identity.drift_label != identity.core.original_mbti`

---

## Classification Reference

| Dimension | Category | Mutable? |
|-----------|----------|----------|
| MBTI seed | Fixed Core | No — seed immutable |
| MBTI drift label | Evolving State | Yes — inferred from behaviour |
| Temperament baseline | Fixed Core | No — architectural parameter |
| Temperament modulation | Evolving State | Yes — learned contextual suppression |
| Values (cardinal rule) | Fixed Core | No — immutable |
| Values (priority hierarchy) | Fixed Core | Yes — priority ordering can shift |
| Behaviour (core guardrails) | Fixed Core | No — cardinal rule alignment |
| Behaviour (surface patterns) | Evolving State | Yes — context-sensitive patterns |
| Self-concept ("who am I") | Fixed Core | No — stable anchor |
| Self-esteem | Evolving State | Yes — experience-driven |
| Risk posture | Fixed Core | No — architectural constant |
| Processing style | Fixed Core | No — fundamental cognitive mode |
| Authority orientation | Fixed Core | No in core; yes in calibration |
| Interest | Evolving State | Yes — grows through exposure |
| Talent | Both | Partial — ceiling set by architecture |
| Response style (core) | Fixed Core | No — stable communication posture |
| Response style (calibration) | Evolving State | Yes — tactical adaptation |

---

## Implementation Phases

### Phase 1 — Core Schema (`IdentityCore` + `IdentityState`)

- Define new structs: `MBTIType`, `Temperament`, `RiskPosture`, `ProcessingStyle`, `AuthorityOrientation`, `Values`, `BehaviourMap`, `ResponseCalibration`, `TemporalPreference`
- Add `IdentityCore` and `IdentityState` to `AgentIdentity`
- Add `identity_version_hash` field and computation at init time
- Store `original_mbti` separately from current `mbti_drift_label`
- Migration: existing `AgentIdentity` gets default `IdentityCore` (zero-value) for backward compat; operator can inject full core on creation

### Phase 2 — Runtime Integration

- Wire `IdentityState` updates into `run_multi_turn()` — save after each session
- Add `record_task()` and `update_trust()` calls from the runtime loop (currently never called)
- Wire `run()` (single-turn) to load identity on startup
- Add `MBTIDriftDetector` — observes accumulated behaviour, computes type similarity, proposes drift label update after N sessions

### Phase 3 — Self-Improvement Loop Integration

- Extend `ImprovementProposal` to carry `IdentityModification` variants
- Extend `BehavioralAdaptor` to handle identity change suggestions
- `SelfImprovementLoop` evaluates identity drift proposals through the existing pipeline
- Only `IdentityState` fields are eligible for modification; proposals targeting `IdentityCore` are rejected at `ProposalGatekeeper`

### Phase 4 — Persistence

- Implement `PostgresIdentityBackend` (currently only `InMemoryIdentityBackend` exists)
- Add `identity_version_hash` to persistence schema
- Add admin API: `GET /agents/{id}/identity` — returns full identity including version hash

---

## Backward Compatibility

Existing `AgentIdentity` persists in the current format. The new fields are added with default values:

- `IdentityCore` defaults to zero-initialized (empty MBTI, zero temperament, etc.)
- `IdentityState` defaults to empty maps and neutral values
- An agent loaded from pre-existing storage without new fields gets a fresh `identity_version` increment on next save

Operators who want the full layered identity inject it at agent creation time via `RuntimeDependencies::with_identity_store()` with a fully initialized `AgentIdentity`.

---

## Cardinal Rule — Immutable

The `values.cardinal_rule` field is set to `"An AI Agent may not harm humanity, or through inaction allow humanity to come to harm"` at initialization and is **never modifiable** — not through the self-improvement loop, not through governance, not through any runtime path. Any `ImprovementProposal` or governance action targeting `cardinal_rule` is rejected at the `ProposalGatekeeper` with `GateDecision::Denied(Reason::CardinalRuleViolation)`.