//! Trace context propagation for containerized and tool workloads.
//!
//! This module bridges the worker's execution layer with downstream observability
//! systems by generating environment variable vectors that are injected into:
//! - Container runtimes (via the worker runtime module)
//! - External tool processes (via the tool executor)
//!
//! It is responsible for carrying identity metadata (tenant, agent, tool) and
//! OpenTelemetry exporter configuration across process boundaries so that traces
//! emitted inside containers remain correlated with the gateway request that
//! initiated them.
//!
//! # 3-tier architecture context
//! This module lives in the **worker (execution layer)**. It receives identity
//! parameters from the gateway layer and ensures they reach the core
//! observability collectors. The returned vectors are consumed by the runtime
//! module when building container specs and by the tool executor when spawning
//! child processes.

/// Builds the environment variable vector for a container workload.
///
/// Injects Clawz identity metadata and OpenTelemetry exporter settings so that
/// the container runtime can report traces and logs correlated to the correct
/// tenant and agent. Called by the worker runtime before starting a container.
///
/// # Arguments
/// - `tenant_id` — Identifier for the tenant that owns the agent.
/// - `agent_id` — Unique identifier for the running agent.
/// - `config_version` — Monotonic version of the agent configuration; used to
///   detect stale containers and ensure observability tags match the active config.
///
/// # Returns
/// A vector of `(String, String)` tuples suitable for passing to a container
/// runtime's environment block.
///
/// # Environment variables injected
/// | Key                        | Purpose                                          |
/// |----------------------------|--------------------------------------------------|
/// | `CLAWZ_TENANT_ID`          | Tenant scoping for multi-tenant trace filtering  |
/// | `CLAWZ_AGENT_ID`           | Agent correlation in distributed traces          |
/// | `CLAWZ_CONFIG_VERSION`     | Config freshness tag for debugging drift         |
/// | `OTEL_SERVICE_NAME`        | OpenTelemetry service name (includes `agent_id`)   |
/// | `OTEL_EXPORTER_OTLP_ENDPOINT` | Hardcoded collector endpoint for worker env     |
/// | `RUST_LOG`                 | Default log level for Rust-based containers        |
pub fn trace_env_for_container(
    tenant_id: &str,
    agent_id: &str,
    config_version: u64,
) -> Vec<(String, String)> {
    vec![
        // Identity metadata — allows the container to self-identify when
        // emitting telemetry so the collector can route/label correctly.
        ("CLAWZ_TENANT_ID".into(), tenant_id.into()),
        ("CLAWZ_AGENT_ID".into(), agent_id.into()),
        // WHY u64 → String: container runtimes expect env values as strings;
        // config_version is numeric in the domain model but must be textual here.
        ("CLAWZ_CONFIG_VERSION".into(), config_version.to_string()),
        // Service name embeds agent_id so each agent appears as a distinct
        // service in the trace backend, making per-agent filtering trivial.
        (
            "OTEL_SERVICE_NAME".into(),
            format!("clawz-agent-{}", agent_id),
        ),
        // Hardcoded to the in-cluster collector because workers run inside
        // the same k8s namespace as the otel-collector sidecar.
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
            "http://otel-collector:4317".into(),
        ),
        // Default log level for Rust containers; downstream executors may override.
        ("RUST_LOG".into(), "info".into()),
    ]
}

/// Builds the environment variable vector for an external tool invocation.
///
/// Similar to [`trace_env_for_container`], but tailored for ephemeral tool
/// processes rather than long-lived containers. Adds tool-specific identity
/// keys so the tool can include its own ID and type in emitted spans.
///
/// # Arguments
/// - `tenant_id` — Tenant that owns the agent invoking the tool.
/// - `agent_id` — Agent under which the tool is being executed.
/// - `tool_id` — Unique identifier for this specific tool instance.
/// - `tool_type` — Canonical tool type / kind (e.g. "bash", "python", "http").
///
/// # Returns
/// A vector of `(String, String)` tuples passed as environment variables to
/// the spawned tool process.
///
/// # Environment variables injected
/// | Key                        | Purpose                                          |
/// |----------------------------|--------------------------------------------------|
/// | `CLAWZ_TENANT_ID`          | Multi-tenant trace scoping                       |
/// | `CLAWZ_AGENT_ID`           | Agent correlation                                  |
/// | `CLAWZ_TOOL_ID`            | Tool instance correlation                          |
/// | `CLAWZ_TOOL_TYPE`          | Tool categorization in telemetry backends          |
/// | `OTEL_SERVICE_NAME`        | Service name derived from `tool_type`                |
/// | `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP collector endpoint                          |
/// | `RUST_LOG`                 | Default log level                                  |
pub fn trace_env_for_tool(
    tenant_id: &str,
    agent_id: &str,
    tool_id: &str,
    tool_type: &str,
) -> Vec<(String, String)> {
    vec![
        // Core identity propagated from the gateway → worker → tool process.
        // Dependency: tenant_id and agent_id originate in the gateway API layer.
        ("CLAWZ_TENANT_ID".into(), tenant_id.into()),
        ("CLAWZ_AGENT_ID".into(), agent_id.into()),
        // Tool-scoped metadata so the collector can group traces by tool
        // instance and type, enabling per-tool latency/error analysis.
        ("CLAWZ_TOOL_ID".into(), tool_id.into()),
        ("CLAWZ_TOOL_TYPE".into(), tool_type.into()),
        // WHY format with tool_type: multiple tools share a binary image;
        // differentiating by type keeps service meshes clean in Jaeger/Tempo.
        (
            "OTEL_SERVICE_NAME".into(),
            format!("clawz-tool-{}", tool_type),
        ),
        // Dependency: otel-collector sidecar address is provisioned by the
        // infrastructure layer; this string must stay in sync with the Helm chart.
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
            "http://otel-collector:4317".into(),
        ),
        ("RUST_LOG".into(), "info".into()),
    ]
}

#[cfg(test)]
mod tests {
    // Dependency: pulls in the two public helpers defined above.
    use super::*;

    /// Verifies that `trace_env_for_container` generates the expected Clawz
    /// identity keys and that values are correctly propagated.
    #[test]
    fn generates_trace_env_vars() {
        let vars = trace_env_for_container("tenant-1", "agent-1", 42);

        // Sanity-check presence of identity keys.
        assert!(vars.iter().any(|(k, _)| k == "CLAWZ_TENANT_ID"));
        assert!(vars.iter().any(|(k, _)| k == "CLAWZ_AGENT_ID"));
        assert!(vars.iter().any(|(k, _)| k == "CLAWZ_CONFIG_VERSION"));

        // Verify value propagation — catches accidental key/value misalignment.
        assert!(
            vars.iter()
                .any(|(k, v)| k == "CLAWZ_TENANT_ID" && v == "tenant-1")
        );
    }

    /// Ensures OpenTelemetry exporter configuration is present in the returned
    /// vector so containers can actually ship traces.
    #[test]
    fn otel_env_included() {
        let vars = trace_env_for_container("t", "a", 1);
        assert!(vars.iter().any(|(k, _)| k == "OTEL_SERVICE_NAME"));
    }
}
