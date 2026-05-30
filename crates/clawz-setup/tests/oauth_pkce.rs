//! OAuth broker unit tests (no network).

use clawz_setup::{oauth_redirect_uri, SetupOAuthProvider};

#[test]
fn oauth_redirect_uri_defaults_to_local_gateway() {
    std::env::remove_var("CLAWZ_SETUP_OAUTH_REDIRECT_URI");
    std::env::remove_var("CLAWZ_PUBLIC_URL");
    let uri = oauth_redirect_uri();
    assert!(uri.contains("/api/v1/setup/oauth/callback"));
}

#[test]
fn provider_parse_rejects_unknown() {
    assert!(SetupOAuthProvider::parse("unknown").is_none());
    assert_eq!(
        SetupOAuthProvider::parse("anthropic").map(|p| p.as_str()),
        Some("anthropic")
    );
}
