use std::collections::BTreeMap;

use serde::{de::Deserializer, Deserialize, Serialize};
use serde_json::Value;

fn string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Value::deserialize(deserializer)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

fn optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Value::deserialize(deserializer)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned))
}

fn string_vec_or_default<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Value::deserialize(deserializer)?
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default())
}

fn optional_reset<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Null => None,
        Value::String(value) => Some(value),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    #[serde(rename = "type")]
    #[serde(default, deserialize_with = "string_or_default")]
    pub provider_type: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub email: String,
    #[serde(
        rename = "refreshToken",
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub refresh_token: Option<String>,
    #[serde(
        rename = "apiKey",
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub api_key: Option<String>,
    #[serde(
        default,
        deserialize_with = "string_vec_or_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub services: Vec<String>,
    #[serde(
        rename = "projectId",
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_id: Option<String>,
    #[serde(
        rename = "managedProjectId",
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub managed_project_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub alias: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub group: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Account {
    pub fn is_supported(&self) -> bool {
        matches!(
            self.provider_type.as_str(),
            "github_copilot" | "openai" | "opencode" | "openrouter"
        )
    }

    pub fn validate(&self) -> bool {
        !self.email.is_empty() && !self.provider_type.is_empty()
    }

    pub fn identity(&self) -> &str {
        if self.provider_type == "github_copilot" {
            self.extra
                .get("github_account")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(&self.email)
        } else {
            &self.email
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(
        rename = "reset",
        alias = "reset_time",
        default,
        deserialize_with = "optional_reset",
        skip_serializing_if = "Option::is_none"
    )]
    pub reset_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub name: String,
    pub elapsed_ms: f64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quota_json_is_sparse_and_uses_public_reset_name() {
        let quota = Quota {
            name: "Primary".into(),
            display_name: "Primary".into(),
            remaining_pct: Some(80.0),
            reset_time: Some("2024-01-01T00:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(quota).unwrap(),
            json!({
                "name": "Primary",
                "display_name": "Primary",
                "remaining_pct": 80.0,
                "reset": "2024-01-01T00:00:00Z"
            })
        );
    }

    #[test]
    fn quota_cache_accepts_numeric_and_older_reset_values() {
        for (value, expected) in [
            (json!(1704067200), "1704067200"),
            (json!("1704067200"), "1704067200"),
            (json!({}), ""),
        ] {
            if value.is_object() {
                assert_eq!(
                    serde_json::from_value::<Quota>(value).unwrap().reset_time,
                    None
                );
            } else {
                assert_eq!(
                    serde_json::from_value::<Quota>(json!({"reset": value}))
                        .unwrap()
                        .reset_time
                        .as_deref(),
                    Some(expected)
                );
            }
        }
        assert_eq!(
            serde_json::from_value::<Quota>(json!({"reset_time": "legacy"}))
                .unwrap()
                .reset_time
                .as_deref(),
            Some("legacy")
        );
    }
}
