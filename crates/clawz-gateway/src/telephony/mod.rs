//! Telephony helpers — Twilio signature validation and TwiML generation.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::BTreeMap;

type HmacSha1 = Hmac<Sha1>;

/// Validate Twilio `X-Twilio-Signature` (HMAC-SHA1 over URL + sorted params).
pub fn verify_twilio_signature(
    auth_token: &str,
    url: &str,
    params: &BTreeMap<String, String>,
    signature: &str,
) -> bool {
    let mut data = url.to_string();
    for (k, v) in params {
        data.push_str(k);
        data.push_str(v);
    }

    let Ok(mut mac) = HmacSha1::new_from_slice(auth_token.as_bytes()) else {
        return false;
    };
    mac.update(data.as_bytes());
    let computed = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        mac.finalize().into_bytes(),
    );
    computed == signature
}

/// Build TwiML that speaks a response and gathers another speech utterance.
pub fn twiml_say_and_gather(say: &str, gather_action_url: &str) -> String {
    let escaped = xml_escape(say);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say>{escaped}</Say>
  <Gather input="speech" action="{gather_action_url}" method="POST" speechTimeout="auto"/>
</Response>"#
    )
}

/// Empty TwiML acknowledgement (SMS webhooks).
pub fn twiml_empty() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?><Response></Response>"#
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Twilio HMAC-SHA1 vector: token `12345`, URL + sorted params From/To.
    #[test]
    fn verify_twilio_signature_accepts_known_vector() {
        let auth_token = "12345";
        let url = "https://example.com/webhook";
        let mut params = BTreeMap::new();
        params.insert("From".into(), "+1234567890".into());
        params.insert("To".into(), "+0987654321".into());
        let signature = "FfwvkTm82C4veRAoZ7E4Yus0rQE=";

        assert!(verify_twilio_signature(auth_token, url, &params, signature));
    }

    #[test]
    fn verify_twilio_signature_rejects_wrong_token_or_signature() {
        let url = "https://example.com/webhook";
        let mut params = BTreeMap::new();
        params.insert("From".into(), "+1234567890".into());
        params.insert("To".into(), "+0987654321".into());
        let signature = "FfwvkTm82C4veRAoZ7E4Yus0rQE=";

        assert!(!verify_twilio_signature("wrong-token", url, &params, signature));
        assert!(!verify_twilio_signature("12345", url, &params, "not-valid-base64-sig="));
    }

    #[test]
    fn twiml_say_and_gather_escapes_xml_special_chars() {
        let say = "Tom & Jerry say \"Hi\" <3 'bye'";
        let gather_url = "https://example.com/gather?x=1&y=2";
        let xml = twiml_say_and_gather(say, gather_url);

        assert!(xml.contains(
            "<Say>Tom &amp; Jerry say &quot;Hi&quot; &lt;3 &apos;bye&apos;</Say>"
        ));
        assert!(xml.contains(r#"action="https://example.com/gather?x=1&y=2""#));
        assert!(xml.contains(r#"<Gather input="speech""#));
    }
}
