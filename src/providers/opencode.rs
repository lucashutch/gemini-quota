use super::base::*;
use crate::model::{Account, Quota, Timing};
use anyhow::{bail, Context};
use serde_json::Value;
use std::time::{Duration, Instant};

const USAGE: &str = "https://opencode.ai/zen/go/v1/usage";

pub struct OpenCodeProvider {
    a: Account,
    t: Vec<Timing>,
}

impl OpenCodeProvider {
    pub fn new(a: Account) -> Self {
        Self { a, t: vec![] }
    }

    pub fn parse_usage(value: &Value) -> Vec<Quota> {
        [
            ("rolling", "Rolling"),
            ("weekly", "Weekly"),
            ("monthly", "Monthly"),
        ]
        .into_iter()
        .filter_map(|(key, label)| {
            let window = value.get("usage")?.get(key)?;
            let used = window
                .get("percent")
                .and_then(Value::as_f64)
                .or_else(|| {
                    window
                        .get("percent")
                        .and_then(Value::as_str)
                        .and_then(|value| value.trim().parse().ok())
                })?
                .clamp(0., 100.);
            let mut quota = quota(&format!("OpenCode Go {label}"), label, "OpenCode Go");
            quota.used_pct = Some(used);
            quota.remaining_pct = Some(100. - used);
            quota.reset_time = window.get("resetsAt").and_then(normalize_reset);
            extra(&mut quota, "window", key);
            extra(&mut quota, "endpoint", "zen/go/v1/usage");
            Some(quota)
        })
        .collect()
    }

    fn api_key(&self) -> Option<&str> {
        self.a
            .api_key
            .as_deref()
            .or_else(|| self.a.extra.get("apiKey").and_then(Value::as_str))
            .or_else(|| self.a.extra.get("api_key").and_then(Value::as_str))
            .filter(|key| !key.trim().is_empty())
    }

    fn usage(
        client: &dyn HttpClient,
        context: &RequestContext,
        api_key: &str,
    ) -> anyhow::Result<HttpResponse> {
        checked(
            client,
            context,
            HttpRequest {
                method: "GET",
                url: USAGE.into(),
                headers: bearer(api_key),
                body: None,
                timeout: Duration::from_secs(10),
            },
        )
    }

    fn error_message(response: &HttpResponse) -> Option<&str> {
        response
            .body
            .get("error")
            .and_then(|error| error.get("message"))
            .or_else(|| response.body.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
    }

    fn require_usage_success(
        response: HttpResponse,
        operation: &str,
    ) -> anyhow::Result<HttpResponse> {
        if (200..300).contains(&response.status) {
            return Ok(response);
        }
        if response.status == 401 {
            bail!("Unauthorized: Invalid OpenCode Go API key");
        }
        let entitlement_error = response
            .body
            .get("error")
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("EntitlementError"));
        if entitlement_error {
            bail!("OpenCode Go subscription required. Subscribe to OpenCode Go and try again.");
        }
        if let Some(message) = Self::error_message(&response) {
            bail!("{operation} failed: {}", sanitize_diagnostic(message));
        }
        require_success(response, operation)
    }
}

impl Provider for OpenCodeProvider {
    fn account(&self) -> &Account {
        &self.a
    }

    fn provider_type(&self) -> &'static str {
        "opencode"
    }

    fn provider_name(&self) -> &'static str {
        "OpenCode Go"
    }

    fn source_priority(&self) -> u8 {
        2
    }

    fn primary_color(&self) -> &'static str {
        "cyan"
    }

    fn short_indicator(&self) -> char {
        'G'
    }

    fn login<'a>(
        &'a mut self,
        input: Value,
        client: &'a dyn HttpClient,
        _: &'a dyn ProcessRunner,
        context: &'a RequestContext,
    ) -> ProviderFuture<'a, Account> {
        Box::pin(async move {
            let key = input["apiKey"]
                .as_str()
                .or_else(|| input["api_key"].as_str())
                .or(self.a.api_key.as_deref())
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .context("API key is required for OpenCode Go login")?;
            let response = Self::usage(client, context, key)?;
            Self::require_usage_success(response, "OpenCode Go login")?;
            let mut account = self.a.clone();
            account.api_key = Some(key.into());
            account.email = input["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("OpenCode Go")
                .into();
            Ok(account)
        })
    }

    fn fetch<'a>(
        &'a mut self,
        client: &'a dyn HttpClient,
        _: &'a dyn ProcessRunner,
        context: &'a RequestContext,
    ) -> ProviderFuture<'a, Vec<Quota>> {
        Box::pin(async move {
            let started = Instant::now();
            let key = self
                .api_key()
                .context("OpenCode Go API key missing; log in again")?;
            let response = Self::usage(client, context, key)?;
            self.t.push(Timing {
                name: "opencode_usage".into(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.,
                extra: Default::default(),
            });
            self.t.push(Timing {
                name: "opencode_total".into(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.,
                extra: Default::default(),
            });
            Self::require_usage_success(response, "OpenCode Go usage")
                .map(|response| Self::parse_usage(&response.body))
        })
    }

    fn sort_key(&self, quota: &Quota) -> (u8, u8, String) {
        let window = match quota.extra.get("window").and_then(Value::as_str) {
            Some("rolling") => 0,
            Some("weekly") => 1,
            Some("monthly") => 2,
            _ => 3,
        };
        (0, window, quota.name.clone())
    }

    fn color(&self, quota: &Quota) -> &'static str {
        match quota.remaining_pct.unwrap_or(100.) {
            value if value >= 50. => "cyan",
            value if value >= 20. => "yellow",
            _ => "red",
        }
    }

    fn timings(&self) -> Vec<Timing> {
        self.t.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use serde_json::json;
    use std::sync::Mutex;

    struct Http {
        response: Mutex<Option<HttpResponse>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl HttpClient for Http {
        fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.lock().unwrap().take().unwrap())
        }
    }

    struct Process;

    impl ProcessRunner for Process {
        fn run(&self, _: &str, _: &[&str], _: Duration) -> Result<ProcessOutput> {
            Ok(ProcessOutput::default())
        }
    }

    #[test]
    fn parse_usage_maps_windows_and_normalizes_resets() {
        let quotas = OpenCodeProvider::parse_usage(&json!({
            "usage": {
                "rolling": {"percent": 12, "resetsAt": "2026-08-13T12:00:00+02:00"},
                "weekly": {"percent": "8", "resetsAt": 1786312800},
                "monthly": {"percent": 135}
            }
        }));
        assert_eq!(quotas.len(), 3);
        assert_eq!(quotas[0].name, "OpenCode Go Rolling");
        assert_eq!(quotas[0].used_pct, Some(12.));
        assert_eq!(quotas[0].remaining_pct, Some(88.));
        assert_eq!(
            quotas[0].reset_time.as_deref(),
            Some("2026-08-13T10:00:00Z")
        );
        assert_eq!(
            quotas[1].reset_time.as_deref(),
            Some("2026-08-09T22:00:00Z")
        );
        assert_eq!(quotas[2].used_pct, Some(100.));
    }

    #[test]
    fn parse_usage_ignores_missing_or_invalid_windows() {
        let quotas = OpenCodeProvider::parse_usage(&json!({
            "usage": {"rolling": {"percent": "not-a-number"}, "weekly": {}}
        }));
        assert!(quotas.is_empty());
    }

    #[test]
    fn login_explains_when_a_valid_key_lacks_a_go_subscription() {
        let http = Http {
            response: Mutex::new(Some(HttpResponse {
                status: 403,
                headers: Default::default(),
                body: json!({
                    "type": "error",
                    "error": {
                        "type": "EntitlementError",
                        "message": "OpenCode Go subscription required."
                    }
                }),
            })),
            requests: Mutex::new(vec![]),
        };
        let mut provider = OpenCodeProvider::new(Account {
            provider_type: "opencode".into(),
            email: "pending".into(),
            ..Default::default()
        });

        let error = futures::executor::block_on(provider.login(
            json!({"apiKey": "sk-test"}),
            &http,
            &Process,
            &RequestContext::default(),
        ))
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "OpenCode Go subscription required. Subscribe to OpenCode Go and try again."
        );
        let request = http.requests.lock().unwrap().pop().unwrap();
        assert_eq!(request.url, USAGE);
        assert_eq!(request.headers["Authorization"], "Bearer sk-test");
    }
}
