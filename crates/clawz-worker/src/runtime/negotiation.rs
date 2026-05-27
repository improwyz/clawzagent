//! Peer-to-peer multi-round negotiation between agents.
//!
//! Unlike one-shot council voting, this protocol allows agents to counter-propose
//! back and forth until either agreement is reached, a party withdraws, or the
//! maximum round limit triggers escalation.

use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

/// A single message in a negotiation exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NegotiationMessage {
    Propose { resource: String, amount: f32 },
    CounterPropose { resource: String, offered: f32, requested: f32 },
    Accept { resource: String, final_amount: f32 },
    Withdraw { resource: String },
    Escalate { resource: String, reason: String },
}

/// An active negotiation session between two agents.
#[derive(Debug, Clone)]
pub(crate) struct NegotiationSession {
    #[allow(dead_code)]
    id: String,
    initiator: String,
    responder: String,
    resource: String,
    round: usize,
    messages: Vec<(String, DateTime<Utc>, NegotiationMessage)>,
}

impl NegotiationSession {
    fn new(id: String, initiator: String, responder: String, resource: String) -> Self {
        Self {
            id,
            initiator,
            responder,
            resource,
            round: 0,
            messages: Vec::new(),
        }
    }

    fn record(&mut self, sender: &str, msg: NegotiationMessage) {
        self.round += 1;
        self.messages.push((sender.to_string(), Utc::now(), msg));
    }

    fn is_participant(&self, agent_id: &str) -> bool {
        agent_id == self.initiator || agent_id == self.responder
    }

    fn counterparty(&self, agent_id: &str) -> Option<String> {
        if agent_id == self.initiator {
            Some(self.responder.clone())
        } else if agent_id == self.responder {
            Some(self.initiator.clone())
        } else {
            None
        }
    }
}

/// Manages multi-round peer-to-peer negotiation.
/// Key property: agents can counter-propose back and forth, unlike one-shot voting.
pub struct NegotiationProtocol {
    max_rounds: usize,
    trust_threshold: f32,
    sessions: RwLock<HashMap<String, NegotiationSession>>,
}

impl NegotiationProtocol {
    /// Create a protocol with default options: 5 max rounds, 0.7 trust threshold.
    pub fn new() -> Self {
        Self::new_with_options(5, 0.7)
    }

    /// Create a protocol with custom options.
    pub fn new_with_options(max_rounds: usize, trust_threshold: f32) -> Self {
        Self {
            max_rounds,
            trust_threshold,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Start a new negotiation. Returns a session ID.
    pub async fn start(
        &self,
        initiator_id: &str,
        responder_id: &str,
        resource: &str,
    ) -> Result<String, clawz_core::error::ClawzError> {
        if initiator_id == responder_id {
            return Err(clawz_core::error::ClawzError::Validation(
                "initiator and responder must be different agents".to_string(),
            ));
        }
        let session_id = Uuid::new_v4().to_string();
        let session = NegotiationSession::new(
            session_id.clone(),
            initiator_id.to_string(),
            responder_id.to_string(),
            resource.to_string(),
        );
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), session);
        Ok(session_id)
    }

    /// Process a message from a participant. Returns the response message.
    pub async fn process(
        &self,
        sender_id: &str,
        session_id: &str,
        msg: NegotiationMessage,
    ) -> Result<NegotiationMessage, clawz_core::error::ClawzError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| {
                clawz_core::error::ClawzError::NotFound {
                    entity: "session".to_string(),
                    id: session_id.to_string(),
                }
            })?;

        // Reject messages from non-participants
        if !session.is_participant(sender_id) {
            return Err(clawz_core::error::ClawzError::Auth(format!(
                "agent {sender_id} is not a participant in session {session_id}"
            )));
        }

        let counterparty = session
            .counterparty(sender_id)
            .expect("participant must have a counterparty");

        // Record the incoming message
        session.record(sender_id, msg.clone());

        // Trading logic
        let response = match msg {
            NegotiationMessage::Propose { resource, amount } => {
                // Initiator proposes → responder counter-proposes with adjusted amounts
                // Default: offer 80% of requested, request 120% of offered
                let offered = amount * 0.8;
                let requested = amount * 1.2;
                NegotiationMessage::CounterPropose {
                    resource,
                    offered,
                    requested,
                }
            }

            NegotiationMessage::CounterPropose { resource, offered, requested } => {
                // Check trust threshold: auto-accept if offered >= threshold * requested
                if offered >= self.trust_threshold * requested {
                    NegotiationMessage::Accept {
                        resource,
                        final_amount: offered,
                    }
                } else if session.round >= self.max_rounds {
                    // Max rounds reached → escalate
                    NegotiationMessage::Escalate {
                        resource,
                        reason: format!(
                            "max rounds ({}) reached without agreement",
                            self.max_rounds
                        ),
                    }
                } else {
                    // Counter-propose back with slight concessions
                    let new_offered = offered * 1.05;
                    let new_requested = requested * 0.95;
                    NegotiationMessage::CounterPropose {
                        resource,
                        offered: new_offered,
                        requested: new_requested,
                    }
                }
            }

            NegotiationMessage::Accept { resource, final_amount } => {
                // Agreement reached — echo acceptance back
                NegotiationMessage::Accept {
                    resource,
                    final_amount,
                }
            }

            NegotiationMessage::Withdraw { resource } => {
                // Withdrawal — echo withdrawal to counterparty
                NegotiationMessage::Withdraw { resource }
            }

            NegotiationMessage::Escalate { .. } => {
                // Already escalated — nothing to do
                return Ok(NegotiationMessage::Escalate {
                    resource: session.resource.clone(),
                    reason: "session already escalated".to_string(),
                });
            }
        };

        // Record the response (round already incremented above)
        let idx = session.messages.len() - 1;
        session.messages[idx] = (counterparty.clone(), Utc::now(), response.clone());

        Ok(response)
    }

    /// Get current session state.
    #[allow(dead_code)]
    pub(crate) async fn session(&self, session_id: &str) -> Option<NegotiationSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }
}

impl Default for NegotiationProtocol {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn two_round_exchange_with_counter_propose() {
        // Two-round: propose → counter-propose → accept
        let protocol = NegotiationProtocol::new_with_options(5, 0.8);
        let session_id = protocol
            .start("alice", "bob", "compute_units")
            .await
            .unwrap();

        // Alice proposes 100 units
        let resp = protocol
            .process("alice", &session_id, NegotiationMessage::Propose {
                resource: "compute_units".to_string(),
                amount: 100.0,
            })
            .await
            .unwrap();

        // Bob counter-proposes: offered=80, requested=120
        let NegotiationMessage::CounterPropose { offered, requested, .. } = resp else {
            panic!("expected CounterPropose, got {resp:?}");
        };
        assert!((offered - 80.0).abs() < 1e-4, "offered = {offered}, expected 80.0");
        assert!((requested - 120.0).abs() < 1e-4, "requested = {requested}, expected 120.0");

        // Bob's counter-propose: offered=80, requested=120
        // Auto-accept since 80 >= 0.8 * 120 = 96 → 80 < 96, so not auto-accept
        // Let's check trust threshold: 80 >= 0.8 * 120 = 96? No. So another counter-propose.
        let resp2 = protocol
            .process("bob", &session_id, NegotiationMessage::CounterPropose {
                resource: "compute_units".to_string(),
                offered,
                requested,
            })
            .await
            .unwrap();

        // Alice's counter-propose back: 80 * 1.05 = 84, 120 * 0.95 = 114
        let NegotiationMessage::CounterPropose { offered: offered2, requested: requested2, .. } = resp2 else {
            panic!("expected CounterPropose, got {resp2:?}");
        };
        assert!((offered2 - 84.0).abs() < 1e-4, "offered2 = {offered2}, expected 84.0");
        assert!((requested2 - 114.0).abs() < 1e-4, "requested2 = {requested2}, expected 114.0");

        // Alice accepts the counter-propose
        let resp3 = protocol
            .process("alice", &session_id, NegotiationMessage::Accept {
                resource: "compute_units".to_string(),
                final_amount: 84.0,
            })
            .await
            .unwrap();

        assert!(matches!(resp3, NegotiationMessage::Accept { .. }));
    }

    #[tokio::test]
    async fn max_rounds_triggers_escalation() {
        let protocol = NegotiationProtocol::new_with_options(2, 0.7);

        let session_id = protocol
            .start("alice", "bob", "memory")
            .await
            .unwrap();

        // Round 1: Alice proposes
        let resp = protocol
            .process("alice", &session_id, NegotiationMessage::Propose {
                resource: "memory".to_string(),
                amount: 100.0,
            })
            .await
            .unwrap();
        assert!(matches!(resp, NegotiationMessage::CounterPropose { .. }));

        // Round 2: Bob counter-proposes (triggering escalation since round >= max_rounds)
        let resp2 = protocol
            .process("bob", &session_id, NegotiationMessage::CounterPropose {
                resource: "memory".to_string(),
                offered: 50.0,
                requested: 200.0,
            })
            .await
            .unwrap();

        let NegotiationMessage::Escalate { resource, reason } = resp2 else {
            panic!("expected Escalate, got {resp2:?}");
        };
        assert_eq!(resource, "memory");
        assert!(reason.contains("max rounds"));
    }

    #[tokio::test]
    async fn same_party_rejected() {
        let protocol = NegotiationProtocol::new();

        let session_id = protocol
            .start("alice", "bob", "storage")
            .await
            .unwrap();

        // Try to send a message as a non-participant
        let err = protocol
            .process("charlie", &session_id, NegotiationMessage::Propose {
                resource: "storage".to_string(),
                amount: 50.0,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, clawz_core::error::ClawzError::Auth(_)));
    }

    #[tokio::test]
    async fn auto_accept_when_trust_threshold_met() {
        let protocol = NegotiationProtocol::new_with_options(5, 0.75);

        let session_id = protocol
            .start("alice", "bob", "bandwidth")
            .await
            .unwrap();

        // Alice proposes 100
        let resp = protocol
            .process("alice", &session_id, NegotiationMessage::Propose {
                resource: "bandwidth".to_string(),
                amount: 100.0,
            })
            .await
            .unwrap();
        // Bob counter-proposes: offered=80, requested=120
        let NegotiationMessage::CounterPropose { offered, requested, .. } = resp else {
            panic!("expected CounterPropose");
        };
        assert!((offered - 80.0).abs() < 1e-4, "offered = {offered}, expected 80.0");
        assert!((requested - 120.0).abs() < 1e-4, "requested = {requested}, expected 120.0");

        // Alice's counter-propose: 80 >= 0.75 * 120 = 90? No → another counter-propose, not auto-accept
        let resp2 = protocol
            .process("alice", &session_id, NegotiationMessage::CounterPropose {
                resource: "bandwidth".to_string(),
                offered,
                requested,
            })
            .await
            .unwrap();

        // 80 >= 90? false, round=2 < max=5 → counter-propose again: 80*1.05=84, 120*0.95=114
        let NegotiationMessage::CounterPropose { offered: o2, requested: r2, .. } = resp2 else {
            panic!("expected CounterPropose, got {resp2:?}");
        };
        assert!((o2 - 84.0).abs() < 1e-4, "o2 = {o2}, expected 84.0");
        assert!((r2 - 114.0).abs() < 1e-4, "r2 = {r2}, expected 114.0");

        // Now check: 84 >= 0.75 * 114 = 85.5? false → counter-propose again
        let resp3 = protocol
            .process("bob", &session_id, NegotiationMessage::CounterPropose {
                resource: "bandwidth".to_string(),
                offered: o2,
                requested: r2,
            })
            .await
            .unwrap();

        // 84*1.05=88.2, 114*0.95=108.3 → counter-propose
        let NegotiationMessage::CounterPropose { offered: o3, .. } = resp3 else {
            panic!("expected CounterPropose, got {resp3:?}");
        };
        assert_eq!(o3, 88.2);

        // Round 4: Alice responds
        let resp4 = protocol
            .process("alice", &session_id, NegotiationMessage::CounterPropose {
                resource: "bandwidth".to_string(),
                offered: o3,
                requested: 108.3,
            })
            .await
            .unwrap();

        // 88.2 >= 0.75 * 108.3 = 81.225? Yes → auto-accept
        let NegotiationMessage::Accept { final_amount, .. } = resp4 else {
            panic!("expected Accept, got {resp4:?}");
        };
        assert_eq!(final_amount, 88.2);
    }

    #[tokio::test]
    async fn withdraw_ends_session() {
        let protocol = NegotiationProtocol::new();

        let session_id = protocol
            .start("alice", "bob", "gpu_time")
            .await
            .unwrap();

        // Alice proposes
        let _resp = protocol
            .process("alice", &session_id, NegotiationMessage::Propose {
                resource: "gpu_time".to_string(),
                amount: 50.0,
            })
            .await
            .unwrap();

        // Bob withdraws
        let resp2 = protocol
            .process("bob", &session_id, NegotiationMessage::Withdraw {
                resource: "gpu_time".to_string(),
            })
            .await
            .unwrap();

        assert!(matches!(resp2, NegotiationMessage::Withdraw { .. }));
    }

    #[tokio::test]
    async fn start_same_agent_fails() {
        let protocol = NegotiationProtocol::new();

        let err = protocol
            .start("alice", "alice", "resource")
            .await
            .unwrap_err();

        assert!(matches!(err, clawz_core::error::ClawzError::Validation(_)));
    }
}