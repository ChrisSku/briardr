// Usage report types, in the JSON shape the client reads and usage.json stores, and the reading of
// the providers' answers that both providers share.
//
// A key the client may find missing (`credits?:`) is an `Option<Option<T>>`: `None` leaves the key
// out, `Some(None)` writes `null`.
use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};

use crate::config::present;

/// Writes a whole number without a fraction (`12`, not `12.0`, which is how the providers send many
/// of them), as the answers and usage.json have always had them.
fn whole<S: Serializer>(n: &f64, s: S) -> Result<S::Ok, S::Error> {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        s.serialize_i64(*n as i64)
    } else {
        s.serialize_f64(*n)
    }
}

fn whole_or_null<S: Serializer>(n: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match n {
        Some(n) => whole(n, s),
        None => s.serialize_none(),
    }
}

/// Epoch milliseconds of a time the providers send in epoch seconds; one that isn't after 1970 is
/// unknown.
pub fn epoch_ms(secs: f64) -> Option<u64> {
    (secs > 0.0).then(|| (secs * 1000.0).round() as u64)
}

/// An object's members in the order they're written. A member whose value isn't a `T` is left out:
/// the providers add members of other shapes (and `null` ones) over time.
#[derive(Debug, Default)]
pub struct Members<T>(pub Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Members<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Member<T> {
            Is(T),
            Other(IgnoredAny),
        }
        struct Object<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
            type Value = Members<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Members<T>, A::Error> {
                let mut members = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    if let Member::Is(value) = map.next_value()? {
                        members.push((key, value));
                    }
                }
                Ok(Members(members))
            }
        }
        d.deserialize_map(Object(PhantomData))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: String,
    pub label: String,
    /// 0-100 (may exceed 100).
    #[serde(serialize_with = "whole")]
    pub used_percent: f64,
    /// Epoch ms when this window resets.
    pub resets_at: Option<u64>,
    /// How long the window is, in minutes: Codex's reports have the key, `null` when it didn't say.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub window_minutes: Option<Option<u64>>,
}

/// Codex's credits, as it sends them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Credits {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

/// Codex's limit reset credits: how many there are, and the available ones it listed (None when it
/// sent only the count).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResetCredits {
    pub available: u64,
    pub credits: Option<Vec<ResetCredit>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResetCredit {
    pub title: Option<String>,
    pub description: Option<String>,
    /// Epoch ms.
    pub granted_at: Option<u64>,
    /// Epoch ms; None when it doesn't expire.
    pub expires_at: Option<u64>,
}

/// Claude's paid usage past the plan's limits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Spend {
    #[serde(serialize_with = "whole_or_null")]
    pub used_usd: Option<f64>,
    #[serde(serialize_with = "whole_or_null")]
    pub limit_usd: Option<f64>,
    pub period: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReport {
    /// "claude" | "codex"
    pub provider: String,
    /// "oauth" | "statusline" | "app-server"
    pub source: String,
    pub windows: Vec<UsageWindow>,
    /// Codex's.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub credits: Option<Option<Credits>>,
    /// Codex's.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub reset_credits: Option<Option<ResetCredits>>,
    /// Claude's.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub spend: Option<Option<Spend>>,
    /// Codex's plan ("plus", "pro", ...).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan: Option<Option<String>>,
    /// Which of Codex's limits was reached ("rate_limit_reached", ...).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub limit_reached: Option<Option<String>>,
    /// When the numbers were captured at the source (epoch ms). Kept across restarts.
    pub captured_at: u64,
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
    use serde_json::{json, Value};

    #[test]
    fn whole_numbers_are_written_without_a_fraction() {
        let window = |used: f64| {
            serde_json::to_string(&UsageWindow {
                id: "a".into(),
                label: "b".into(),
                used_percent: used,
                resets_at: None,
                window_minutes: None,
            })
            .unwrap()
        };
        assert!(window(12.0).contains(r#""usedPercent":12,"#));
        assert!(window(41.5).contains(r#""usedPercent":41.5,"#));
        let spend = Spend {
            used_usd: Some(1234.0),
            limit_usd: None,
            period: None,
        };
        assert_eq!(
            serde_json::to_string(&spend).unwrap(),
            r#"{"usedUsd":1234,"limitUsd":null,"period":null}"#
        );
    }

    #[test]
    fn epoch_seconds_become_milliseconds() {
        assert_eq!(epoch_ms(1_791_741_600.0), Some(1_791_741_600_000));
        assert_eq!(epoch_ms(0.0), None);
        assert_eq!(epoch_ms(-5.0), None);
    }

    #[test]
    fn members_keep_their_order_and_skip_other_shapes() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Limit {
            used: f64,
        }
        let m: Members<Limit> = serde_json::from_value(json!({
            "b": {"used": 1}, "flag": true, "a": {"used": 2.5}, "off": null, "other": {"x": 1}
        }))
        .unwrap();
        let keys: Vec<&str> = m.0.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(m.0[1].1, Limit { used: 2.5 });
        assert!(serde_json::from_value::<Members<Limit>>(json!([1])).is_err());
    }

    #[test]
    fn optional_keys_round_trip() {
        let src = json!({"provider":"codex","source":"app-server","windows":[{"id":"a","label":"b","usedPercent":1,"resetsAt":null,"windowMinutes":null}],"credits":null,"capturedAt":5});
        let r: ProviderReport = serde_json::from_value(src.clone()).unwrap();
        assert_eq!(r.credits, Some(None));
        assert_eq!(r.spend, None);
        assert_eq!(serde_json::to_value(&r).unwrap(), src);
        let v: Value = serde_json::to_value(&r).unwrap();
        assert!(v.get("spend").is_none());
    }
}
