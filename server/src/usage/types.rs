// Usage report types, in the JSON shape the client reads and usage.json stores.
//
// Several fields pass source values through unchecked (`x ?? null`, `resets_at * 1000`), so they
// are `Value` here: the JSON the client sees and the persisted usage.json keep their existing shape.
// Optional keys (`key?:`) are `Option<Value>`: `None` = key absent, `Some(Value::Null)` = `null`.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// A JS number as JSON: whole numbers serialize as integers (like `JSON.stringify`), NaN/±∞ as null.
pub fn js_num(n: f64) -> Value {
    if !n.is_finite() {
        Value::Null
    } else if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        Value::from(n as i64)
    } else {
        Value::from(n)
    }
}

/// Deserializes a present key, including `null`, as `Some`; a missing key stays `None` via `default`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: String,
    pub label: String,
    /// 0-100 (may exceed 100), or null.
    pub used_percent: Value,
    /// Epoch ms when this window resets, or null.
    pub resets_at: Value,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub window_minutes: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReport {
    /// "claude" | "codex"
    pub provider: String,
    /// "oauth" | "statusline" | "app-server"
    pub source: String,
    pub windows: Vec<UsageWindow>,
    /// `{hasCredits, unlimited, balance}` or null.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub credits: Option<Value>,
    /// `{available, credits: [{title, description, grantedAt, expiresAt}] | null}` or null.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub reset_credits: Option<Value>,
    /// `{usedUsd, limitUsd, period}` or null.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub spend: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub limit_reached: Option<Value>,
    /// When the numbers were captured at the source (epoch ms). Kept across restarts.
    pub captured_at: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LastError {
    pub category: String,
    pub message: String,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
/// Missing keys take the defaults, so a partial saved object loads.
#[serde(rename_all = "camelCase", default)]
pub struct ProviderStatus {
    pub report: Option<ProviderReport>,
    pub last_error: Option<LastError>,
    pub last_attempt_at: Option<u64>,
    /// Earliest time another request may be made (rate-limit cooldown or failure backoff).
    pub cooldown_until: Option<u64>,
    pub rate_limited_until: Option<u64>,
    pub failures: u32,
    pub enabled: bool,
}

impl Default for ProviderStatus {
    fn default() -> Self {
        ProviderStatus {
            report: None,
            last_error: None,
            last_attempt_at: None,
            cooldown_until: None,
            rate_limited_until: None,
            failures: 0,
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_num_matches_json_stringify() {
        assert_eq!(js_num(12.0), json!(12));
        assert_eq!(js_num(41.5), json!(41.5));
        assert_eq!(js_num(f64::NAN), Value::Null);
        assert_eq!(js_num(1_791_741_600_000.0).to_string(), "1791741600000");
    }

    #[test]
    fn optional_keys_round_trip() {
        let src = json!({"provider":"codex","source":"app-server","windows":[{"id":"a","label":"b","usedPercent":1,"resetsAt":null,"windowMinutes":null}],"credits":null,"capturedAt":5});
        let r: ProviderReport = serde_json::from_value(src.clone()).unwrap();
        assert_eq!(r.credits, Some(Value::Null));
        assert_eq!(r.spend, None);
        assert_eq!(serde_json::to_value(&r).unwrap(), src);
    }
}
