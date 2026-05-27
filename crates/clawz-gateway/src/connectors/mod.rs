//! # Connector Registry
//!
//! This module aggregates all 23 SaaS connector implementations and provides a central
//! [`ConnectorRegistry`] for runtime lookup by platform identifier. The gateway uses this
//! registry to route incoming HTTP requests to the correct connector based on the target
//! platform.
//!
//! ## Module Organization
//!
//! - `common` — Shared HTTP helpers, OAuth2 flow state machine, and token parsing.
//! - `trait` — The [`SaaSConnector`] trait and supporting types ([`AuthType`], [`Credentials`], [`Filters`]).
//! - Individual connector modules — One per supported SaaS platform.
//!
//! ## Adding a New Connector
//!
//! 1. Create a new module file implementing [`SaaSConnector`].
//! 2. Export it here with `pub mod <name>;`.
//! 3. Re-export the concrete type with `pub use <name>::<Type>;`.
//! 4. Register it in the gateway's bootstrap code.
//!
//! // Dependency: `crate::connectors::common` provides `ApiClient`, `OAuth2Flow`, `TokenResponse`.
//! // Dependency: `crate::connectors::trait` defines the `SaaSConnector` contract all modules implement.

pub mod common;
pub mod r#trait;

// Existing connectors
pub mod atlassian;
pub mod github;
pub mod google_workspace;
pub mod hubspot;
pub mod microsoft365;
pub mod salesforce;
pub mod slack;
pub mod stripe;

// CRM & Sales
pub mod freshsales;
pub mod pipedrive;
pub mod zoho;

// Productivity & Collaboration
pub mod asana;
pub mod clickup;
pub mod linear;
pub mod monday;
pub mod notion;

// Communication
pub mod intercom;
pub mod sendgrid;
pub mod twilio;

// Finance & Payments
pub mod quickbooks;
pub mod xero;

// Marketing & Analytics
pub mod google_analytics;
pub mod mailchimp;
pub mod meta_ads;

// Developer & DevOps
pub mod aws;
pub mod datadog;
pub mod gitlab;
pub mod pagerduty;

// E-Commerce
pub mod shopify;

// Storage & Documents
pub mod box_com;
pub mod dropbox;

// Re-exports from common
pub use common::{ApiClient, OAuth2Flow, TokenResponse};

// Re-exports for existing connectors
pub use atlassian::AtlassianConnector;
pub use github::GitHubConnector;
pub use google_workspace::GoogleWorkspaceConnector;
pub use hubspot::HubSpotConnector;
pub use microsoft365::Microsoft365Connector;
pub use salesforce::SalesforceConnector;
pub use slack::SlackConnector;
pub use stripe::StripeConnector;

// Re-exports for new connectors
pub use asana::AsanaConnector;
pub use aws::AwsConnector;
pub use box_com::BoxConnector;
pub use clickup::ClickUpConnector;
pub use datadog::DatadogConnector;
pub use dropbox::DropboxConnector;
pub use freshsales::FreshsalesConnector;
pub use gitlab::GitLabConnector;
pub use google_analytics::GoogleAnalyticsConnector;
pub use intercom::IntercomConnector;
pub use linear::LinearConnector;
pub use mailchimp::MailchimpConnector;
pub use meta_ads::MetaAdsConnector;
pub use monday::MondayConnector;
pub use notion::NotionConnector;
pub use pagerduty::PagerDutyConnector;
pub use pipedrive::PipedriveConnector;
pub use quickbooks::QuickBooksConnector;
pub use sendgrid::SendGridConnector;
pub use shopify::ShopifyConnector;
pub use twilio::TwilioConnector;
pub use xero::XeroConnector;
pub use zoho::ZohoConnector;

// Trait re-exports
pub use r#trait::{AuthType, Credentials, Filters, SaaSConnector};

use std::collections::HashMap;
use std::sync::Arc;

/// Registry for looking up connectors by platform identifier.
///
/// The registry owns a map from platform ID strings (e.g. `"github"`, `"salesforce"`)
/// to shared, type-erased [`SaaSConnector`] instances. It is cheaply cloneable via `Arc`
/// and is typically populated once at startup.
pub struct ConnectorRegistry {
    /// Map from platform identifier to the connector implementation.
    ///
    /// Each entry is an `Arc<dyn SaaSConnector>` so connectors can be shared across
    /// async request handlers without cloning the underlying connection state.
    connectors: HashMap<String, Arc<dyn SaaSConnector>>,
}

impl ConnectorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            connectors: HashMap::new(),
        }
    }

    /// Register a connector under its platform ID.
    ///
    /// The platform ID is derived from [`SaaSConnector::platform_id`] at insertion time.
    /// If a connector with the same ID already exists, it is replaced.
    pub fn register(&mut self, connector: Arc<dyn SaaSConnector>) {
        let id = connector.platform_id().to_string();
        self.connectors.insert(id, connector);
    }

    /// Look up a connector by platform ID.
    ///
    /// Returns `None` if no connector has been registered for the given ID.
    pub fn get(&self, platform_id: &str) -> Option<Arc<dyn SaaSConnector>> {
        self.connectors.get(platform_id).cloned()
    }

    /// List all registered platform IDs.
    pub fn platform_ids(&self) -> Vec<&str> {
        self.connectors.keys().map(|s| s.as_str()).collect()
    }

    /// Number of registered connectors.
    pub fn len(&self) -> usize {
        self.connectors.len()
    }

    /// Returns true if no connectors are registered.
    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
