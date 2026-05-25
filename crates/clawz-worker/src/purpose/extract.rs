//! Extract structured data from natural-language goal strings.
//!
//! `Extractor` uses regex heuristics to pull out metrics, dates, and target
//! values so that downstream validation and parsing have concrete data to
//! work with.

use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;

/// Heuristic extractor for goal-string metadata.
#[derive(Debug, Clone)]
pub struct Extractor;

impl Extractor {
    /// Create a new extractor.
    pub fn new() -> Self {
        Self
    }

    /// Extract numeric metrics mentioned in the input.
    ///
    /// Returns a list of `(metric_name, value)` pairs found by simple
    /// regex patterns such as `"latency under 100ms"` or `"cost < $50"`.
    ///
    /// # Examples
    /// ```
    /// use clawz_worker::purpose::Extractor;
    /// let ex = Extractor::new();
    /// let metrics = ex.extract_metrics("reduce latency under 100ms and cost below $50");
    /// assert!(metrics.iter().any(|(k, v)| k == "latency" && *v == 100.0));
    /// assert!(metrics.iter().any(|(k, v)| k == "cost" && *v == 50.0));
    /// ```
    pub fn extract_metrics(&self, input: &str) -> Vec<(String, f64)> {
        let mut results = Vec::new();
        let lower = input.to_lowercase();

        // Pattern: <word> [under|below|<|less than] <number> [optional unit]
        let re_under = Regex::new(
            r"([a-z_]+)\s*(?:under|below|less than|<)\s*\$?(\d+(?:\.\d+)?)"
        ).unwrap();
        for cap in re_under.captures_iter(&lower) {
            if let (Some(name), Some(num)) = (cap.get(1), cap.get(2)) {
                if let Ok(v) = num.as_str().parse::<f64>() {
                    results.push((name.as_str().to_string(), v));
                }
            }
        }

        // Pattern: <word> [over|above|>|more than] <number>
        let re_over = Regex::new(
            r"([a-z_]+)\s*(?:over|above|more than|>)\s*\$?(\d+(?:\.\d+)?)"
        ).unwrap();
        for cap in re_over.captures_iter(&lower) {
            if let (Some(name), Some(num)) = (cap.get(1), cap.get(2)) {
                if let Ok(v) = num.as_str().parse::<f64>() {
                    results.push((name.as_str().to_string(), v));
                }
            }
        }

        // Pattern: <word> of <number> or <word> = <number>
        let re_exact = Regex::new(
            r"([a-z_]+)\s*(?:of|is|=)\s*\$?(\d+(?:\.\d+)?)"
        ).unwrap();
        for cap in re_exact.captures_iter(&lower) {
            if let (Some(name), Some(num)) = (cap.get(1), cap.get(2)) {
                if let Ok(v) = num.as_str().parse::<f64>() {
                    // Avoid duplicates
                    if !results.iter().any(|(n, _)| n == name.as_str()) {
                        results.push((name.as_str().to_string(), v));
                    }
                }
            }
        }

        results
    }

    /// Extract ISO-8601 or common-language dates from the input.
    ///
    /// Supports `YYYY-MM-DD` and a few spoken forms like `"by 2025-06-01"`.
    ///
    /// # Examples
    /// ```
    /// use clawz_worker::purpose::Extractor;
    /// let ex = Extractor::new();
    /// let dates = ex.extract_dates("finish by 2025-06-01");
    /// assert_eq!(dates.len(), 1);
    /// ```
    pub fn extract_dates(&self, input: &str) -> Vec<DateTime<Utc>> {
        let mut results = Vec::new();
        let re = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
        for cap in re.captures_iter(input) {
            if let Some(m) = cap.get(1) {
                if let Ok(nd) = NaiveDate::parse_from_str(m.as_str(), "%Y-%m-%d") {
                    results.push(DateTime::from_naive_utc_and_offset(
                        nd.and_hms_opt(0, 0, 0).unwrap(),
                        Utc,
                    ));
                }
            }
        }
        results
    }

    /// Extract a target entity or objective string from the input.
    ///
    /// Heuristic: take the first sentence, stripped of leading verbs.
    ///
    /// # Examples
    /// ```
    /// use clawz_worker::purpose::Extractor;
    /// let ex = Extractor::new();
    /// assert_eq!(ex.extract_target("Deploy the service to prod."), "the service to prod");
    /// ```
    pub fn extract_target(&self, input: &str) -> String {
        let first_sentence = input.split('.').next().unwrap_or(input).trim();
        let lower = first_sentence.to_lowercase();
        let stripped = lower
            .strip_prefix("deploy ")
            .or_else(|| lower.strip_prefix("build "))
            .or_else(|| lower.strip_prefix("create "))
            .or_else(|| lower.strip_prefix("fix "))
            .or_else(|| lower.strip_prefix("reduce "))
            .or_else(|| lower.strip_prefix("increase "))
            .or_else(|| lower.strip_prefix("minimize "))
            .or_else(|| lower.strip_prefix("maximize "))
            .or_else(|| lower.strip_prefix("optimize "))
            .or_else(|| lower.strip_prefix("explore "))
            .or_else(|| lower.strip_prefix("maintain "))
            .or_else(|| lower.strip_prefix("satisfy "))
            .unwrap_or(&lower);
        stripped.trim().to_string()
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_metrics_finds_latency_and_cost() {
        let ex = Extractor::new();
        let metrics = ex.extract_metrics("reduce latency under 100ms and cost below $50");
        assert!(metrics.iter().any(|(k, v)| k == "latency" && *v == 100.0));
        assert!(metrics.iter().any(|(k, v)| k == "cost" && *v == 50.0));
    }

    #[test]
    fn extract_metrics_finds_above_threshold() {
        let ex = Extractor::new();
        let metrics = ex.extract_metrics("keep uptime above 99.9");
        assert!(metrics.iter().any(|(k, v)| k == "uptime" && *v == 99.9));
    }

    #[test]
    fn extract_dates_finds_iso_date() {
        let ex = Extractor::new();
        let dates = ex.extract_dates("finish by 2025-06-01");
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0].date_naive().to_string(), "2025-06-01");
    }

    #[test]
    fn extract_dates_finds_multiple_dates() {
        let ex = Extractor::new();
        let dates = ex.extract_dates("start 2025-01-01 and end 2025-12-31");
        assert_eq!(dates.len(), 2);
    }

    #[test]
    fn extract_target_strips_deploy() {
        let ex = Extractor::new();
        assert_eq!(ex.extract_target("Deploy the service to prod."), "the service to prod");
    }

    #[test]
    fn extract_target_defaults_to_full_when_no_verb() {
        let ex = Extractor::new();
        assert_eq!(ex.extract_target("the service to prod"), "the service to prod");
    }
}
