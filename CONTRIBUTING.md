# Contributing to ClawZ

Welcome to ClawZ! We're building a Rust-native orchestration platform for autonomous agents at Enterpryz Ventures. Your contributions help us evolve the framework for real-world agent workloads.

## Project Overview

ClawZ is a Rust 2024 workspace with three core crates:

- **clawz-core**: Type definitions, traits, configuration, and errors shared across the platform
- **clawz-worker**: Container orchestration, governance engines, mesh networking, and agent runtime
- **clawz-gateway**: HTTP API, scheduling, admission control, and request routing

Licensed under **Elastic License 2.0 (ELv2)**. All contributors agree to the Contributor License Agreement.

## Getting Started

### Prerequisites

- **Rust 1.87+** (MSRV enforced in CI)
- **PostgreSQL 14+** with pgvector extension enabled
- **Docker** (optional, for container orchestration testing)
- **Git** and a GitHub account

### Install & run locally

The fastest path is the one-click installer — it clones the repo, creates `.env`, and starts gateway, worker, and Postgres (pgvector):

```bash
git clone https://github.com/improwyz/clawz.git
cd clawz
./scripts/install.sh          # Docker when available, else source build
./scripts/install.sh --source   # Force cargo build without Docker
./scripts/install.sh --with-web # Include React dashboard build
```

See **[INSTALL.md](INSTALL.md)** for Windows (`install.ps1`), production checklist, and troubleshooting.

### Clone & build (without installer)

```bash
git clone https://github.com/improwyz/clawz.git
cd clawz
cp .env.example .env
cargo build --workspace
```

### Run tests

```bash
cargo test --workspace
```

### Database (optional manual setup)

The Docker installer and Compose stack provision Postgres with pgvector automatically. For a custom Postgres instance:

```bash
createdb clawz
psql clawz -c "CREATE EXTENSION IF NOT EXISTS pgvector"
export DATABASE_URL="postgres://user:password@localhost:5432/clawz"
cargo sqlx migrate run
```

## Development Workflow

### Build Commands

```bash
# Build entire workspace
cargo build --workspace

# Run clippy (required for PRs)
cargo clippy --workspace -- -D warnings

# Format code
cargo fmt --all

# Run all tests
cargo test --workspace
```

### Project Structure

```
clawz/
├── crates/
│   ├── clawz-core/          # Traits, types, config, errors
│   ├── clawz-worker/        # Runtime, governance, mesh, scheduling
│   └── clawz-gateway/       # HTTP API, admission control, routing
├── CONTRIBUTING.md
├── Cargo.toml
└── Cargo.lock
```

## How to Contribute

1. **Found a bug?** Open an issue with reproducible steps. Start a discussion before coding.
2. **Have a feature idea?** Open an issue first to discuss scope and approach with maintainers.
3. **Ready to code?** Fork the repository, create a feature branch, and submit a PR.

### Issue Labels

- `bug`: Unexpected behavior
- `feature`: New capability
- `refactor`: Code quality improvement
- `docs`: Documentation updates
- `good first issue`: Approachable for new contributors

## Pull Request Guidelines

- **One concern per PR**: Bug fixes, features, and refactors in separate PRs
- **Keep PRs small**: Aim for <500 lines of logic changes
- **Follow conventional commits**: prefix your branch and commits with `feat/`, `fix/`, `refactor/`, `docs/`, `test/`, or `chore/`
- **Pass CI checks**:
  - `cargo clippy --workspace -- -D warnings`
  - `cargo test --workspace`
  - `cargo fmt --all` (check formatting)
- **Fill the PR template**: Link related issues, explain what and why
- **Request review**: Assign a maintainer for feedback

Example PR:

```
feat: add distributed tracing for agent lifecycle

Implement OpenTelemetry integration in clawz-gateway to trace
agent execution across the mesh. Resolves #123.

- Adds TracingMiddleware to HTTP stack
- Emits span events at checkpoint boundaries
- Configurable trace endpoint via OTEL_EXPORTER_OTLP_ENDPOINT
```

## Code Style & Standards

### Rust Idioms

- Write for **Rust 2024** idioms—prefer iterators, builders, and composition
- Use **async-first** design; blocking calls require explicit justification
- Trait-driven architecture: define capabilities as traits, implement for concrete types
- Error handling via `ClawzError` (no `.unwrap()` in production code)

### Naming & Structure

```rust
// Traits in clawz-core
pub trait Provider {
    async fn execute(&self, req: Request) -> Result<Response, ClawzError>;
}

// Implementations in worker/gateway
impl Provider for DockerProvider {
    // ...
}
```

## Testing

- **Unit tests**: Colocated in modules with `#[cfg(test)]`
- **Integration tests**: In `gateway/tests/` for end-to-end scenarios
- **Async tests**: Use `#[tokio::test]`
- **Mocking**: Mock at trait boundaries, not concrete types

Example:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_lifecycle() {
        let mock_runtime = MockRuntime::new();
        // ...
        assert_eq!(result.status, Status::Completed);
    }
}
```

## Adding New Capabilities

To add a new provider, tool, channel, or plugin:

1. **Define the trait** in `clawz-core/src/traits.rs` (or appropriate module)
2. **Implement for your capability** in the relevant worker/gateway module
3. **Register in the registry**: Update the provider/tool/channel factory
4. **Add tests**: Unit + integration for the happy path
5. **Document**: Add a brief example in code comments

Example: New Provider

```rust
// clawz-core/src/traits.rs
pub trait Provider: Send + Sync {
    async fn execute(&self, request: Request) -> Result<Response, ClawzError>;
}

// clawz-worker/src/providers/custom.rs
pub struct CustomProvider { /* ... */ }
impl Provider for CustomProvider { /* ... */ }

// Register in factory
impl ProviderFactory {
    pub fn register_custom(&mut self, cfg: CustomConfig) -> Result<(), ClawzError> {
        self.providers.insert("custom", Arc::new(CustomProvider::new(cfg)?));
        Ok(())
    }
}
```

## What We Will Not Merge

- **Formatting-only PRs**: Use `cargo fmt` in your editor
- **Speculative features**: Features without a use case or design
- **Governance bypasses**: Changes that circumvent approval policies
- **Undocumented dependencies**: New crates require justification and discussion
- **Breaking changes without migration**: Maintain backward compatibility or provide a clear deprecation path

## License

ClawZ is licensed under **Elastic License 2.0 (ELv2)** (Elastic License 2.0 (ELv2)). By contributing, you agree to license your work under the same terms and sign the Contributor License Agreement.

See [LICENSE.md](./LICENSE.md) for full text.

---

**Questions?** Open an issue or ask in discussions. We're here to help!

Happy coding, and thank you for building ClawZ with us.
