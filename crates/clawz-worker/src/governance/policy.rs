//! Policy engine — pattern matching and condition evaluation for governance rules.
//!
//! This module implements the runtime evaluation of [`GovernancePolicy`] documents.
//! Policies are loaded into a [`PolicyEngine`], which matches incoming actions and
//! their JSON context against each policy's rules.  The first matching rule's
//! [`PolicyEffect`] wins for that policy.
//!
//! # Condition matching language
//!
//! Rules use a small string-based DSL understood by [`PolicyEngine::condition_matches`].
//! Supported patterns (single quotes are required around string literals):
//!
//! | Pattern | Matches when … |
//! |---------|---------------|
//! | `true` | always |
//! | `false` | never |
//! | `action == 'value'` | the action string equals `value` exactly |
//! | `action contains 'value'` | the action string contains `value` as a substring |
//! | `action starts_with 'value'` | the action string starts with `value` |
//! | `field == 'value'` | the JSON context field `field` is a string equal to `value` |
//! | `field contains 'value'` | the JSON context field `field` is a string containing `value` |
//! | `field > threshold` | the JSON context field `field` is a numeric `f64` greater than `threshold` |
//! | `field < threshold` | the JSON context field `field` is a numeric `f64` less than `threshold` |
//!
//! ## Examples
//!
//! ```rust,ignore
//! // Block any action whose name is exactly "deploy"
//! "action == 'deploy'"
//!
//! // Block any action whose name includes the word "delete"
//! "action contains 'delete'"
//!
//! // Trigger when the context field `env` equals "production"
//! "env == 'production'"
//!
//! // Deny requests whose numeric `cost` field exceeds 100.0
//! "cost > 100"
//!
//! // Always match (useful for blanket deny/allow/review policies)
//! "true"
//! ```
//!
//! # Evaluation order
//!
//! Policies are evaluated in **descending priority order** (highest priority first).
//! Only *enabled* policies are considered.  A policy whose conditions do not match
//! is skipped entirely.  Each matching rule generates a [`Violation`] carrying the
//! rule's [`PolicyEffect`] (`Deny` or `Review`).  `Allow` effects produce no
//! violation because they represent explicit clearance.

use std::collections::HashMap;

use clawz_core::types::governance::{GovernancePolicy, PolicyEffect, PolicyRule};
use serde_json::Value;

// ── Violation ─────────────────────────────────────────────────────────────────

/// A recorded policy breach returned by [`PolicyEngine::evaluate`].
///
/// Each `Violation` ties a triggered rule back to its parent policy so that
/// callers can report *what* was denied, *which* policy blocked it, and *why*.
///
/// # Fields
///
/// - `policy_id` — Unique identifier of the [`GovernancePolicy`] that fired.
/// - `policy_name` — Human-readable name of the policy (e.g. "Block production deploys").
/// - `rule_description` — Explanation attached to the specific rule that matched.
/// - `effect` — The [`PolicyEffect`] produced by the matched rule (`Deny` or `Review`).
///
/// # Example
///
/// ```rust,ignore
/// let v = Violation {
///     policy_id: "p1".into(),
///     policy_name: "Cost guard".into(),
///     rule_description: "cost too high".into(),
///     effect: PolicyEffect::Deny,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct Violation {
    pub policy_id: String,
    pub policy_name: String,
    pub rule_description: String,
    /// The effect decided by the matched rule.  Only `Deny` and `Review`
    /// appear in violations because `Allow` explicitly signals clearance
    /// and therefore produces no `Violation`.
    pub effect: PolicyEffect,
}

// ── PolicyEngine ──────────────────────────────────────────────────────────────

/// In-memory registry and evaluator for governance policies.
///
/// `PolicyEngine` holds a collection of [`GovernancePolicy`] documents keyed by
/// their `id`.  It provides CRUD-like helpers to add, remove, enable, and disable
/// policies, as well as the core [`evaluate`](PolicyEngine::evaluate) method that
/// matches an action and its JSON context against every enabled policy.
///
/// # Thread safety
///
/// `PolicyEngine` is **not** `Sync`; if it must be shared across threads,
/// protect it with a mutex or wrap it in an actor.
///
/// # Example
///
/// ```rust,ignore
/// let mut engine = PolicyEngine::new();
/// engine.add_policy(GovernancePolicy::deny_all("lockdown"));
/// let hits = engine.evaluate("chat", &serde_json::json!({}));
/// assert_eq!(hits.len(), 1);
/// ```
pub struct PolicyEngine {
    policies: HashMap<String, GovernancePolicy>,
}

impl PolicyEngine {
    /// Create an empty engine with no policies registered.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let engine = PolicyEngine::new();
    /// assert_eq!(engine.policy_count(), 0);
    /// ```
    pub fn new() -> Self {
        Self {
            policies: HashMap::new(),
        }
    }

    /// Register a new policy, replacing any existing policy with the same `id`.
    ///
    /// The policy is added in whatever state it carries (enabled or disabled).
    /// To activate a disabled policy after insertion, call [`enable_policy`](PolicyEngine::enable_policy).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// engine.add_policy(GovernancePolicy::deny_all("block-everything"));
    /// ```
    pub fn add_policy(&mut self, policy: GovernancePolicy) {
        self.policies.insert(policy.id.clone(), policy);
    }

    /// Remove the policy identified by `id` from the engine.
    ///
    /// Does nothing if no policy with that `id` exists.
    pub fn remove_policy(&mut self, id: &str) {
        self.policies.remove(id);
    }

    /// Enable the policy identified by `id`.
    ///
    /// Enabled policies participate in [`evaluate`](PolicyEngine::evaluate).
    /// Does nothing if the `id` is unknown.
    pub fn enable_policy(&mut self, id: &str) {
        if let Some(p) = self.policies.get_mut(id) {
            p.enabled = true;
        }
    }

    /// Disable the policy identified by `id`.
    ///
    /// Disabled policies are skipped during [`evaluate`](PolicyEngine::evaluate).
    /// Does nothing if the `id` is unknown.
    pub fn disable_policy(&mut self, id: &str) {
        if let Some(p) = self.policies.get_mut(id) {
            p.enabled = false;
        }
    }

    /// Return an immutable reference to the policy with the given `id`, if any.
    pub fn get_policy(&self, id: &str) -> Option<&GovernancePolicy> {
        self.policies.get(id)
    }

    /// Return the number of policies currently stored (including disabled ones).
    pub fn policy_count(&self) -> usize {
        self.policies.len()
    }

    /// Evaluate `action` and its JSON `context` against all **enabled** policies.
    ///
    /// Policies are inspected in descending priority order.  For each policy,
    /// every rule is tested; the first rule whose [`condition_matches`](PolicyEngine::condition_matches)
    /// returns `true` causes the rule's [`PolicyEffect`] to be applied:
    ///
    /// | Effect | Result |
    /// |--------|--------|
    /// | `Deny` | A [`Violation`] is pushed into the returned vector. |
    /// | `Review` | A [`Violation`] is pushed into the returned vector. |
    /// | `Allow` | No violation is recorded (explicit clearance). |
    ///
    /// # Returns
    ///
    /// A `Vec<Violation>` containing every deny/review triggered during evaluation.
    /// An empty vector means the action cleared all policies.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut engine = PolicyEngine::new();
    /// engine.add_policy(GovernancePolicy::deny_all("deny"));
    /// let v = engine.evaluate("any_action", &serde_json::json!({}));
    /// assert_eq!(v.len(), 1);
    /// assert!(matches!(v[0].effect, PolicyEffect::Deny));
    /// ```
    pub fn evaluate(&self, action: &str, context: &Value) -> Vec<Violation> {
        // Sort policies by descending priority.
        let mut sorted: Vec<&GovernancePolicy> =
            self.policies.values().filter(|p| p.enabled).collect();
        sorted.sort_by_key(|p| std::cmp::Reverse(p.priority));

        let mut violations = Vec::new();

        for policy in sorted {
            for rule in &policy.rules {
                if self.condition_matches(rule, action, context) {
                    match rule.effect {
                        PolicyEffect::Deny => {
                            violations.push(Violation {
                                policy_id: policy.id.clone(),
                                policy_name: policy.name.clone(),
                                rule_description: rule.description.clone(),
                                effect: PolicyEffect::Deny,
                            });
                        }
                        PolicyEffect::Review => {
                            violations.push(Violation {
                                policy_id: policy.id.clone(),
                                policy_name: policy.name.clone(),
                                rule_description: rule.description.clone(),
                                effect: PolicyEffect::Review,
                            });
                        }
                        PolicyEffect::Allow => {
                            // Explicit allow — no violation.
                        }
                    }
                }
            }
        }

        violations
    }

    /// Parse a rule's `condition` string and test it against `action`/`context`.
    ///
    /// This is the implementation of the condition matching language documented
    /// at the module level.  Supported patterns are listed below with extra
    /// detail and edge-case notes.
    ///
    /// # Supported patterns
    ///
    /// ## Literal booleans
    ///
    /// - `"true"` — always returns `true`.
    /// - `"false"` — always returns `false`.
    ///
    /// ## Action predicates
    ///
    /// - `"action == 'value'"` — exact equality against the `action` parameter.
    /// - `"action contains 'value'"` — substring search in the `action` parameter.
    /// - `"action starts_with 'value'"` — prefix check on the `action` parameter.
    ///
    /// ## Context field predicates
    ///
    /// - `"field == 'value'"` — looks up `field` in the JSON `context`.  If the
    ///   field is a JSON string and equals `value`, the condition matches.
    /// - `"field contains 'value'"` — looks up `field` in `context`.  If the field
    ///   is a JSON string and contains `value` as a substring, it matches.
    /// - `"field > threshold"` — looks up `field` in `context`.  If the field is a
    ///   JSON number and its `f64` value is strictly greater than `threshold`, it matches.
    /// - `"field < threshold"` — same as above but for strictly less than.
    ///
    /// ## Unknown syntax
    ///
    /// Any condition that does not match the patterns above logs a warning and
    /// is treated as **no-match** (`false`).
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Exact action match
    /// "action == 'deploy'"
    ///
    /// // Substring match on action
    /// "action contains 'delete'"
    ///
    /// // Prefix match on action
    /// "action starts_with 'admin_'"
    ///
    /// // Context field string equality
    /// "env == 'production'"
    ///
    /// // Context field substring
    /// "message contains 'password'"
    ///
    /// // Numeric comparison against context
    /// "cost > 100"
    /// "retry_count < 3"
    ///
    /// // Universal match / block-all
    /// "true"
    /// ```
    fn condition_matches(&self, rule: &PolicyRule, action: &str, context: &Value) -> bool {
        let cond = rule.condition.trim();

        // Literal booleans.
        if cond == "true" {
            return true;
        }
        if cond == "false" {
            return false;
        }

        // `action == 'value'`
        if let Some(rest) = cond.strip_prefix("action == '") {
            let val = rest.trim_end_matches('\'');
            return action == val;
        }

        // `action contains 'value'`
        if let Some(rest) = cond.strip_prefix("action contains '") {
            let val = rest.trim_end_matches('\'');
            return action.contains(val);
        }

        // `action starts_with 'value'`
        if let Some(rest) = cond.strip_prefix("action starts_with '") {
            let val = rest.trim_end_matches('\'');
            return action.starts_with(val);
        }

        // `field == 'value'`
        if let Some(pos) = cond.find(" == '") {
            let field = &cond[..pos];
            let value_part = &cond[pos + 5..];
            let val = value_part.trim_end_matches('\'');
            if let Some(field_val) = context.get(field) {
                return field_val.as_str().map(|s| s == val).unwrap_or(false);
            }
            return false;
        }

        // `field contains 'value'`
        if let Some(pos) = cond.find(" contains '") {
            let field = &cond[..pos];
            let value_part = &cond[pos + 11..];
            let val = value_part.trim_end_matches('\'');
            if let Some(field_val) = context.get(field) {
                return field_val.as_str().map(|s| s.contains(val)).unwrap_or(false);
            }
            return false;
        }

        // `field > threshold`
        if let Some(pos) = cond.find(" > ") {
            let field = &cond[..pos];
            let threshold_str = &cond[pos + 3..];
            if let Ok(threshold) = threshold_str.trim().parse::<f64>() {
                if let Some(field_val) = context.get(field) {
                    return field_val.as_f64().map(|v| v > threshold).unwrap_or(false);
                }
            }
            return false;
        }

        // `field < threshold`
        if let Some(pos) = cond.find(" < ") {
            let field = &cond[..pos];
            let threshold_str = &cond[pos + 3..];
            if let Ok(threshold) = threshold_str.trim().parse::<f64>() {
                if let Some(field_val) = context.get(field) {
                    return field_val.as_f64().map(|v| v < threshold).unwrap_or(false);
                }
            }
            return false;
        }

        // Unknown condition — treat as no-match.
        log::warn!("[policy] unknown condition syntax: '{cond}'");
        false
    }
}

impl Default for PolicyEngine {
    /// Delegates to [`PolicyEngine::new`].
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::governance::GovernancePolicy;

    #[test]
    fn test_no_policies_no_violations() {
        let engine = PolicyEngine::new();
        let violations = engine.evaluate("chat", &serde_json::json!({}));
        assert!(violations.is_empty());
    }

    #[test]
    fn test_deny_all_policy() {
        let mut engine = PolicyEngine::new();
        engine.add_policy(GovernancePolicy::deny_all("test"));
        let violations = engine.evaluate("any_action", &serde_json::json!({}));
        assert_eq!(violations.len(), 1);
        assert!(matches!(violations[0].effect, PolicyEffect::Deny));
    }

    #[test]
    fn test_allow_all_policy_no_violations() {
        let mut engine = PolicyEngine::new();
        engine.add_policy(GovernancePolicy::allow_all("test"));
        let violations = engine.evaluate("anything", &serde_json::json!({}));
        assert!(violations.is_empty());
    }

    #[test]
    fn test_action_equals_condition() {
        let mut engine = PolicyEngine::new();
        let policy = GovernancePolicy::new(
            "p1",
            "block deploy",
            vec![PolicyRule::deny("action == 'deploy'", "no deploy")],
        );
        engine.add_policy(policy);

        assert_eq!(engine.evaluate("deploy", &serde_json::json!({})).len(), 1);
        assert!(engine.evaluate("chat", &serde_json::json!({})).is_empty());
    }

    #[test]
    fn test_action_contains_condition() {
        let mut engine = PolicyEngine::new();
        let policy = GovernancePolicy::new(
            "p2",
            "block delete",
            vec![PolicyRule::deny("action contains 'delete'", "no delete")],
        );
        engine.add_policy(policy);

        assert_eq!(
            engine.evaluate("delete_file", &serde_json::json!({})).len(),
            1
        );
        assert_eq!(
            engine.evaluate("bulk_delete", &serde_json::json!({})).len(),
            1
        );
        assert!(engine.evaluate("create", &serde_json::json!({})).is_empty());
    }

    #[test]
    fn test_disabled_policy_skipped() {
        let mut engine = PolicyEngine::new();
        let mut policy = GovernancePolicy::deny_all("disabled");
        policy.enabled = false;
        engine.add_policy(policy);

        assert!(
            engine
                .evaluate("anything", &serde_json::json!({}))
                .is_empty()
        );
    }

    #[test]
    fn test_context_field_equals() {
        let mut engine = PolicyEngine::new();
        let policy = GovernancePolicy::new(
            "p3",
            "block prod",
            vec![PolicyRule::deny("env == 'production'", "no prod")],
        );
        engine.add_policy(policy);

        assert_eq!(
            engine
                .evaluate("deploy", &serde_json::json!({"env": "production"}))
                .len(),
            1
        );
        assert!(
            engine
                .evaluate("deploy", &serde_json::json!({"env": "staging"}))
                .is_empty()
        );
    }

    #[test]
    fn test_review_effect_recorded() {
        let mut engine = PolicyEngine::new();
        let policy = GovernancePolicy::new(
            "p4",
            "review",
            vec![PolicyRule::review("true", "always review")],
        );
        engine.add_policy(policy);

        let violations = engine.evaluate("anything", &serde_json::json!({}));
        assert_eq!(violations.len(), 1);
        assert!(matches!(violations[0].effect, PolicyEffect::Review));
    }

    #[test]
    fn test_numeric_gt_condition() {
        let mut engine = PolicyEngine::new();
        let policy = GovernancePolicy::new(
            "p5",
            "high cost",
            vec![PolicyRule::deny("cost > 100", "cost too high")],
        );
        engine.add_policy(policy);

        assert_eq!(
            engine
                .evaluate("buy", &serde_json::json!({"cost": 150.0}))
                .len(),
            1
        );
        assert!(
            engine
                .evaluate("buy", &serde_json::json!({"cost": 50.0}))
                .is_empty()
        );
    }
}
