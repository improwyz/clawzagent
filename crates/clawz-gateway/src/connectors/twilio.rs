//! Twilio connector for Clawz Gateway.
//!
//! Integrates with the Twilio REST API (2010-04-01). Supports SMS, voice calls,
//! phone numbers, recordings and WhatsApp messages.
//!
//! # Authentication
//! HTTP Basic Auth using Account SID and Auth Token.
//!
//! # Cross-module dependencies
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Twilio connector (SMS, voice, WhatsApp).
///
/// All requests target `https://api.twilio.com/2010-04-01/Accounts/{account_sid}`.
/// HTTP Basic Auth is injected on every request builder.
pub struct TwilioConnector {
    /// Twilio Account SID (used as the username for Basic Auth).
    account_sid: String,
    /// Twilio Auth Token (used as the password for Basic Auth).
    auth_token: String,
    /// Shared HTTP client.
    client: Client,
}

impl TwilioConnector {
    /// Create a new Twilio connector.
    pub fn new(account_sid: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            account_sid: account_sid.into(),
            auth_token: auth_token.into(),
            client: Client::new(),
        }
    }

    /// Build the per-account Twilio API base URL.
    fn base_url(&self) -> String {
        format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}",
            self.account_sid
        )
    }

    /// Start a GET request with Basic Auth already applied.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.base_url(), path))
            .basic_auth(&self.account_sid, Some(&self.auth_token))
    }

    /// Start a POST request with Basic Auth already applied.
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{}", self.base_url(), path))
            .basic_auth(&self.account_sid, Some(&self.auth_token))
    }

    /// Start a DELETE request with Basic Auth already applied.
    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}{}", self.base_url(), path))
            .basic_auth(&self.account_sid, Some(&self.auth_token))
    }
}

#[async_trait]
impl SaaSConnector for TwilioConnector {
    fn platform_id(&self) -> &str {
        "twilio"
    }

    fn display_name(&self) -> &str {
        "Twilio"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::BasicAuth
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth("Twilio uses Basic authentication".into()))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth("Twilio uses Basic authentication".into()))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        // Twilio allows up to 1000 records per page.
        let page_size = filters.limit.unwrap_or(50).min(1000);
        let path = match obj {
            "messages" => "/Messages.json",
            "calls" => "/Calls.json",
            "phone_numbers" => "/IncomingPhoneNumbers.json",
            "recordings" => "/Recordings.json",
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Twilio object: {obj}"
                )));
            }
        };
        let resp = self
            .get(path)
            .query(&[("PageSize", page_size.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Twilio list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Twilio uses plural snake_case keys that sometimes differ from the object name.
        let key = match obj {
            "messages" => "messages",
            "calls" => "calls",
            "phone_numbers" => "incoming_phone_numbers",
            "recordings" => "recordings",
            _ => obj,
        };
        Ok(json[key].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        match obj {
            "messages" => {
                // Twilio expects form-encoded POST bodies for messages and calls.
                let to = data["to"].as_str().unwrap_or("");
                let from = data["from"].as_str().unwrap_or("");
                let body = data["body"].as_str().unwrap_or("");
                let params = [("To", to), ("From", from), ("Body", body)];
                let resp = self
                    .post("/Messages.json")
                    .form(&params)
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("Twilio send message failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            "calls" => {
                let to = data["to"].as_str().unwrap_or("");
                let from = data["from"].as_str().unwrap_or("");
                let url = data["url"].as_str().unwrap_or("");
                let params = [("To", to), ("From", from), ("Url", url)];
                let resp = self
                    .post("/Calls.json")
                    .form(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Twilio make call failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Twilio object: {obj}"
            ))),
        }
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        match obj {
            "calls" => {
                let status = data["status"].as_str().unwrap_or("completed");
                let params = [("Status", status)];
                let resp = self
                    .post(&format!("/Calls/{id}.json"))
                    .form(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Twilio update call failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Twilio object: {obj}"
            ))),
        }
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "messages" => format!("/Messages/{id}.json"),
            "recordings" => format!("/Recordings/{id}.json"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Twilio object: {obj}"
                )));
            }
        };
        let resp = self
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Twilio delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Twilio delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        match action {
            "send_whatsapp" => {
                // Twilio requires the `whatsapp:` prefix for both To and From.
                let to = format!("whatsapp:{}", params["to"].as_str().unwrap_or(""));
                let from = format!("whatsapp:{}", params["from"].as_str().unwrap_or(""));
                let body = params["body"].as_str().unwrap_or("");
                let form_params = [("To", to.as_str()), ("From", from.as_str()), ("Body", body)];
                let resp = self
                    .post("/Messages.json")
                    .form(&form_params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Twilio WhatsApp failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Twilio action: {action}"
            ))),
        }
    }
}
