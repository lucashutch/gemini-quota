use anyhow::Result;
use limitwatch::{
    model::Account,
    providers::{self, base::*},
};
use serde_json::{json, Value};
use std::{
    fs,
    sync::Mutex,
    time::{Duration, Instant},
};
struct Http {
    responses: Mutex<Vec<HttpResponse>>,
    requests: Mutex<Vec<HttpRequest>>,
}
impl HttpClient for Http {
    fn execute(&self, r: HttpRequest) -> Result<HttpResponse> {
        self.requests.lock().unwrap().push(r);
        Ok(self.responses.lock().unwrap().remove(0))
    }
}
struct Proc;
impl ProcessRunner for Proc {
    fn run(&self, _: &str, _: &[&str], _: Duration) -> Result<ProcessOutput> {
        Ok(ProcessOutput {
            success: true,
            ..Default::default()
        })
    }
}
fn account(kind: &str) -> Account {
    Account {
        provider_type: kind.into(),
        email: "user@example.com".into(),
        api_key: Some("SECRET_KEY".into()),
        refresh_token: Some("SECRET_REFRESH".into()),
        ..Default::default()
    }
}
#[test]
fn every_provider_has_stable_metadata() {
    for kind in ["github_copilot", "openai", "opencode", "openrouter"] {
        let p = providers::create(account(kind)).unwrap();
        assert_eq!(p.provider_type(), kind);
        assert!(!p.provider_name().is_empty());
        assert!(!p.primary_color().is_empty());
        assert!(p.short_indicator().is_ascii_uppercase());
    }
}
#[test]
fn request_context_bounds_and_cancels() {
    let ctx = RequestContext {
        deadline: Some(Instant::now() + Duration::from_millis(20)),
        ..Default::default()
    };
    assert!(ctx.remaining(Duration::from_secs(5)).unwrap() <= Duration::from_millis(20));
    ctx.cancellation.cancel();
    assert!(ctx
        .remaining(Duration::from_secs(1))
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
}

#[test]
fn checked_uses_remaining_absolute_deadline_for_each_request() {
    let http = Http {
        responses: Mutex::new(vec![HttpResponse {
            status: 200,
            body: Value::Null,
            headers: Default::default(),
        }]),
        requests: Mutex::new(vec![]),
    };
    let ctx = RequestContext {
        deadline: Some(Instant::now() + Duration::from_millis(20)),
        ..Default::default()
    };
    checked(
        &http,
        &ctx,
        HttpRequest {
            method: "GET",
            url: "https://example.test".into(),
            headers: Default::default(),
            body: None,
            timeout: Duration::from_secs(10),
        },
    )
    .unwrap();
    let timeout = http.requests.lock().unwrap()[0].timeout;
    assert!(timeout > Duration::ZERO);
    assert!(timeout <= Duration::from_millis(20));
}
#[test]
fn provider_errors_do_not_expose_credentials() {
    let secret = "SECRET_KEY";
    let http = Http {
        responses: Mutex::new(vec![
            HttpResponse {
                status: 401,
                body: Value::Null,
                headers: Default::default(),
            },
            HttpResponse {
                status: 401,
                body: Value::Null,
                headers: Default::default(),
            },
        ]),
        requests: Mutex::new(vec![]),
    };
    let mut p = providers::create(account("openrouter")).unwrap();
    let error = futures::executor::block_on(p.fetch(&http, &Proc, &RequestContext::default()))
        .unwrap_err()
        .to_string();
    assert!(!error.contains(secret));
    assert!(!error.contains("Authorization"));
    assert_eq!(
        http.requests.lock().unwrap()[0].headers["Authorization"],
        format!("Bearer {secret}")
    );
}
#[test]
fn parsers_accept_representative_payloads() {
    use limitwatch::providers::{
        openai::OpenAiProvider, opencode::OpenCodeProvider, openrouter::OpenRouterProvider,
    };
    assert_eq!(OpenAiProvider::parse_usage(&json!({"rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":3600}}})).len(),1);
    assert_eq!(
        OpenRouterProvider::parse_credits(&json!({"data":{"total_credits":10,"total_usage":2}}))
            .remaining,
        Some(8.0)
    );
    assert_eq!(
        OpenCodeProvider::parse_usage(&json!({"usage":{"rolling":{"percent":10}}}))[0]
            .remaining_pct,
        Some(90.0)
    );
}

#[test]
fn opencode_fetches_subscription_usage_with_a_bearer_key() {
    let http = Http {
        responses: Mutex::new(vec![HttpResponse {
            status: 200,
            body: json!({"usage":{"rolling":{"percent":12,"resetsAt":"2026-08-13T12:00:00Z"}}}),
            headers: Default::default(),
        }]),
        requests: Mutex::new(vec![]),
    };
    let mut provider = providers::create(account("opencode")).unwrap();
    let quotas =
        futures::executor::block_on(provider.fetch(&http, &Proc, &RequestContext::default()))
            .unwrap();

    assert_eq!(quotas[0].name, "OpenCode Go Rolling");
    assert_eq!(quotas[0].remaining_pct, Some(88.));
    let request = &http.requests.lock().unwrap()[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.url, "https://opencode.ai/zen/go/v1/usage");
    assert_eq!(request.headers["Authorization"], "Bearer SECRET_KEY");
}

#[test]
fn openrouter_redacted_key_labels_require_a_safe_friendly_name() {
    let http = Http {
        responses: Mutex::new(vec![HttpResponse {
            status: 200,
            body: json!({"data":{"label":"sk-or-v1-abc...xyz"}}),
            headers: Default::default(),
        }]),
        requests: Mutex::new(vec![]),
    };
    let mut provider = providers::create(Account {
        provider_type: "openrouter".into(),
        email: "pending".into(),
        ..Default::default()
    })
    .unwrap();
    let logged_in = futures::executor::block_on(provider.login(
        json!({"apiKey":"sk-or-secret"}),
        &http,
        &Proc,
        &RequestContext::default(),
    ))
    .unwrap();

    assert_eq!(logged_in.email, "OpenRouter Key");
    assert_eq!(logged_in.extra["_limitwatch_openrouter_needs_name"], true);
    assert!(!serde_json::to_string(&logged_in)
        .unwrap()
        .contains("abc...xyz"));
}

#[test]
fn openai_discovery_and_device_progress_are_safe_and_specific() {
    use limitwatch::providers::openai::{
        openai_device_authorization_status, openai_discovery_status,
    };

    assert_eq!(
        openai_discovery_status("opencode", true),
        Some("✓ Found OpenCode token")
    );
    assert_eq!(
        openai_discovery_status("opencode", false),
        Some("⚠ OpenCode token invalid or expired")
    );
    assert_eq!(
        openai_discovery_status("codex", true),
        Some("✓ Found Codex CLI token")
    );
    assert_eq!(
        openai_discovery_status("codex", false),
        Some("⚠ Codex CLI token invalid")
    );
    assert_eq!(openai_discovery_status("unknown", true), None);
    assert_eq!(
        openai_device_authorization_status(),
        "Starting device code authorization..."
    );
}

#[test]
fn reset_and_failure_contract_fixture_matches() {
    let f: Value = serde_json::from_str(include_str!("fixtures/contracts/reference.json")).unwrap();
    for c in f["resets"].as_array().unwrap() {
        assert_eq!(normalize_reset(&c["input"]).unwrap(), c["canonical"]);
    }
    for c in f["failures"].as_array().unwrap() {
        let error = require_success(
            HttpResponse {
                status: c["status"].as_u64().unwrap() as u16,
                body: Value::Null,
                headers: Default::default(),
            },
            "fetch",
        )
        .unwrap_err();
        assert_eq!(error.to_string(), c["message"]);
    }
}

#[test]
fn reset_normalization_accepts_numeric_strings_and_fractional_epochs() {
    assert_eq!(
        normalize_reset(&json!("1700000000")),
        Some("2023-11-14T22:13:20Z".into())
    );
    assert_eq!(
        normalize_reset(&json!(1700000000.5)),
        Some("2023-11-14T22:13:20Z".into())
    );
}

#[test]
fn diagnostic_sanitization_redacts_credentials_and_urls() {
    let message = sanitize_diagnostic(
        "Bearer header-secret token=token-secret https://user:password@example.test/x",
    );
    assert!(!message.contains("header-secret"));
    assert!(!message.contains("token-secret"));
    assert!(!message.contains("password"));
    assert!(!message.contains("example.test"));
}

#[test]
fn openai_local_credentials_and_extended_usage_contract() {
    use limitwatch::providers::openai::OpenAiProvider;

    let path = std::env::temp_dir().join(format!(
        "limitwatch-openai-auth-{}-{}.json",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    fs::write(
        &path,
        json!({
            "access_token": "offline-access-token",
            "refresh_token": "offline-refresh-token",
            "email": "local@example.com"
        })
        .to_string(),
    )
    .unwrap();
    let http = Http {
        responses: Mutex::new(vec![
            HttpResponse {
                status: 200,
                body: json!({"plan_type":"pro"}),
                headers: Default::default(),
            },
            HttpResponse {
                status: 401,
                body: Value::Null,
                headers: Default::default(),
            },
        ]),
        requests: Mutex::new(vec![]),
    };
    let mut provider = OpenAiProvider::new(account("openai"));
    let logged_in = futures::executor::block_on(provider.login(
        json!({"authFile":path.to_string_lossy()}),
        &http,
        &Proc,
        &RequestContext::default(),
    ))
    .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(logged_in.email, "local@example.com");
    assert_eq!(logged_in.extra["refreshToken"], "offline-refresh-token");

    let quotas = OpenAiProvider::parse_usage(&json!({
        "plan_type":"pro",
        "additional_rate_limits":[{"name":"Cloud Tasks","used_percent":25,"limit_window_seconds":1800}],
        "credits":{"has_credits":true,"balance":75.5,"unlimited":false}
    }));
    assert_eq!(quotas.len(), 2);
    assert_eq!(quotas[0].display_name, "Cloud Tasks (30m)");
    assert_eq!(quotas[1].extra["balance"], 75.5);
}

#[test]
fn openrouter_key_fallback_marks_unlimited_keys_as_spend_only() {
    let http = Http {
        responses: Mutex::new(vec![
            HttpResponse {
                status: 403,
                body: Value::Null,
                headers: Default::default(),
            },
            HttpResponse {
                status: 200,
                body: json!({"data":{"label":"unlimited","usage":314.0 / 100.0,"limit":null}}),
                headers: Default::default(),
            },
        ]),
        requests: Mutex::new(vec![]),
    };
    let mut provider = providers::create(account("openrouter")).unwrap();
    let quotas =
        futures::executor::block_on(provider.fetch(&http, &Proc, &RequestContext::default()))
            .unwrap();
    assert_eq!(quotas[0].display_name, "unlimited: $3.14 spent");
    assert_eq!(quotas[0].remaining_pct, Some(100.));
    assert_eq!(quotas[0].limit, Some(0.));
}

#[test]
fn github_selected_identity_and_explicit_org_survive_optional_403s() {
    struct Gh(Mutex<Vec<Vec<String>>>);
    impl ProcessRunner for Gh {
        fn run(&self, command: &str, args: &[&str], _: Duration) -> Result<ProcessOutput> {
            assert_eq!(command, "gh");
            self.0
                .lock()
                .unwrap()
                .push(args.iter().map(|arg| (*arg).to_owned()).collect());
            Ok(ProcessOutput {
                success: true,
                stdout: "selected-token\n".into(),
                stderr: "diagnostic".into(),
            })
        }
    }
    let http = Http {
        responses: Mutex::new(vec![HttpResponse {
            status: 200,
            body: json!({"login":"Selected-User"}),
            headers: Default::default(),
        }]),
        requests: Mutex::new(vec![]),
    };
    let gh = Gh(Mutex::new(vec![]));
    let mut provider = providers::create(account("github_copilot")).unwrap();
    let logged_in = futures::executor::block_on(provider.login(
        json!({"github_account":"selected-user", "organization":"Myriota", "token_source":"gh_cli"}),
        &http,
        &gh,
        &RequestContext::default(),
    ))
    .unwrap();

    assert_eq!(
        gh.0.lock().unwrap()[0],
        ["auth", "token", "--user", "selected-user"]
    );
    let request = &http.requests.lock().unwrap()[0];
    assert_eq!(request.url, "https://api.github.com/user");
    assert_eq!(request.headers["Authorization"], "Bearer selected-token");
    assert_eq!(request.headers["Accept"], "application/vnd.github+json");
    assert_eq!(request.headers["X-GitHub-Api-Version"], "2026-03-10");
    assert_eq!(request.headers["User-Agent"], "limitwatch");
    assert_eq!(logged_in.email, "Selected-User");
    assert_eq!(logged_in.extra["github_account"], "Selected-User");
    assert_eq!(logged_in.extra["github_selected_account"], "selected-user");
    assert_eq!(logged_in.extra["githubAuthSource"], "gh_cli");
    assert_eq!(logged_in.extra["githubToken"], "selected-token");
    assert_eq!(logged_in.extra["organization"], "Myriota");
}

#[test]
fn github_internal_user_is_first_single_call_and_selects_configured_org() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/github_copilot/myriota_internal_user.json"
    ))
    .unwrap();
    let mut responses = vec![HttpResponse {
        status: 200,
        body: fixture,
        headers: Default::default(),
    }];
    responses.extend((0..4).map(|_| HttpResponse {
        status: 404,
        body: json!({"message":"Not Found"}),
        headers: Default::default(),
    }));
    let http = Http {
        responses: Mutex::new(responses),
        requests: Mutex::new(vec![]),
    };
    let mut saved = account("github_copilot");
    saved.email = "user".into();
    saved
        .extra
        .insert("githubToken".into(), json!("sanitized-token"));
    saved.extra.insert("organization".into(), json!("myriota"));
    let mut provider = providers::create(saved).unwrap();
    let quotas =
        futures::executor::block_on(provider.fetch(&http, &Proc, &RequestContext::default()))
            .unwrap();
    let org = quotas.iter().find(|q| q.display_name == "myriota").unwrap();
    assert!(!quotas.iter().any(|q| q.display_name == "Personal"));
    assert_eq!(org.name, "GitHub Copilot Org (myriota)");
    assert_eq!(org.limit, Some(26000.0));
    assert_eq!(org.used, Some(231.3));
    assert_eq!(org.remaining, Some(25768.7));
    assert!((org.used_pct.unwrap() - 0.8896153846153846).abs() < f64::EPSILON);
    assert!((org.remaining_pct.unwrap() - 99.11038461538462).abs() < f64::EPSILON);
    assert_eq!(org.reset_time.as_deref(), Some("2026-08-01T00:00:00Z"));
    let requests = http.requests.lock().unwrap();
    assert_eq!(
        requests[0].url,
        "https://api.github.com/copilot_internal/user"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.ends_with("/copilot_internal/user"))
            .count(),
        1
    );
    assert_eq!(requests.len(), 1);
}

#[test]
fn github_internal_user_malformed_or_no_match_falls_back_to_billing() {
    for body in [
        json!({"quota_snapshots":null}),
        json!({
            "organization_login_list":["OtherOrg"],
            "quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":68.7}}
        }),
    ] {
        let missing = || HttpResponse {
            status: 404,
            body: json!({"message":"Not Found"}),
            headers: Default::default(),
        };
        let mut responses = vec![HttpResponse {
            status: 200,
            body,
            headers: Default::default(),
        }];
        responses.extend((0..8).map(|_| missing()));
        let http = Http {
            responses: Mutex::new(responses),
            requests: Mutex::new(vec![]),
        };
        let mut saved = account("github_copilot");
        saved.email = "user".into();
        saved.extra.insert("githubToken".into(), json!("token"));
        saved.extra.insert("organization".into(), json!("Myriota"));
        let mut provider = providers::create(saved).unwrap();
        let quotas =
            futures::executor::block_on(provider.fetch(&http, &Proc, &RequestContext::default()))
                .unwrap();
        assert_eq!(
            quotas
                .iter()
                .find(|q| q.display_name == "Myriota")
                .unwrap()
                .extra["is_error"],
            true
        );
        let requests = http.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.ends_with("/copilot_internal/user"))
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.contains("/orgs/Myriota/settings/billing/usage"))
                .count(),
            4
        );
    }
}

#[test]
fn github_reloaded_work_account_fetches_only_myriota_endpoints() {
    let usage = json!({"usageItems":[{"product":"Copilot premium requests","grossQuantity":2.0}]});
    let http = Http {
        responses: Mutex::new(vec![
            HttpResponse {
                status: 200,
                body: json!({"copilot_plan":"pro"}),
                headers: Default::default(),
            },
            HttpResponse {
                status: 200,
                body: usage,
                headers: Default::default(),
            },
            HttpResponse {
                status: 200,
                body: json!({"usageItems":[{"product":"Copilot premium requests","grossQuantity":2.0}]}),
                headers: Default::default(),
            },
            HttpResponse {
                status: 200,
                body: json!({"seat_breakdown":{"total":2},"plan_type":"business"}),
                headers: Default::default(),
            },
        ]),
        requests: Mutex::new(vec![]),
    };
    let mut saved = account("github_copilot");
    saved.email = "Lucashutch".into();
    saved
        .extra
        .insert("github_account".into(), json!("Lucashutch"));
    saved
        .extra
        .insert("githubToken".into(), json!("validated-token"));
    saved.extra.insert("organization".into(), json!("Myriota"));
    let reloaded: Account = serde_json::from_value(serde_json::to_value(saved).unwrap()).unwrap();
    let mut provider = providers::create(reloaded).unwrap();
    let quotas =
        futures::executor::block_on(provider.fetch(&http, &Proc, &RequestContext::default()))
            .unwrap();
    assert_eq!(quotas.len(), 1);
    let requests = http.requests.lock().unwrap();
    assert_eq!(
        requests[0].url,
        "https://api.github.com/copilot_internal/user"
    );
    assert!(requests[1]
        .url
        .contains("/orgs/Myriota/settings/billing/usage?"));
    assert_eq!(
        requests[2].url,
        "https://api.github.com/orgs/Myriota/copilot/billing"
    );
    assert!(requests
        .iter()
        .all(|request| !request.url.contains("/users/Lucashutch/")));
    for request in requests.iter() {
        assert!(request.headers["Authorization"].contains("validated-token"));
        assert_eq!(request.headers["Accept"], "application/vnd.github+json");
        assert!(request.headers.contains_key("X-GitHub-Api-Version"));
    }
}

#[test]
fn github_billing_request_sequence() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/github_copilot/myriota_billing_requests.json"
    ))
    .unwrap();
    let empty = || HttpResponse {
        status: 200,
        body: json!({"usageItems":[]}),
        headers: Default::default(),
    };
    let usage = || HttpResponse {
        status: 200,
        body: json!({"usageItems":[{"product":"Copilot AI credits","grossQuantity":2.0}]}),
        headers: Default::default(),
    };
    let mut responses = vec![HttpResponse {
        status: 200,
        body: json!({"copilot_plan":"pro"}),
        headers: Default::default(),
    }];
    responses.extend([empty(), empty(), usage()]);
    responses.extend([empty(), empty(), usage()]);
    responses.push(HttpResponse {
        status: 200,
        body: json!({"seat_breakdown":{"total":2},"plan_type":"business"}),
        headers: Default::default(),
    });
    let http = Http {
        responses: Mutex::new(responses),
        requests: Mutex::new(vec![]),
    };
    let mut saved = account("github_copilot");
    saved.email = "Lucashutch".into();
    saved
        .extra
        .insert("githubToken".into(), json!("validated-token"));
    saved.extra.insert("organization".into(), json!("Myriota"));
    let mut provider = providers::create(saved).unwrap();
    futures::executor::block_on(provider.fetch(&http, &Proc, &RequestContext::default())).unwrap();

    let requests = http.requests.lock().unwrap();
    assert_eq!(
        requests[0].url,
        "https://api.github.com/copilot_internal/user"
    );
    let billing = requests
        .iter()
        .filter(|r| r.url.contains("/settings/billing/usage"))
        .collect::<Vec<_>>();
    for (index, request) in billing.iter().enumerate() {
        let (path, query) = request
            .url
            .strip_prefix("https://api.github.com")
            .unwrap()
            .split_once('?')
            .unwrap();
        assert_eq!(path, fixture["billing_paths"][index + 3]);
        let parts = query.split('&').collect::<Vec<_>>();
        let expected = fixture["queries"][index + 3].as_str().unwrap();
        assert_eq!(
            &parts[..2]
                .iter()
                .map(|p| p.split('=').next().unwrap())
                .collect::<Vec<_>>(),
            &["year", "month"]
        );
        if expected == "filtered" {
            assert!(parts[2].starts_with("start_date="));
            assert!(parts[3].starts_with("end_date="));
            assert!(query.contains("product=copilot&sku=copilot_ai_credits"));
        } else {
            assert!(query.contains("start_date=") && query.contains("end_date="));
        }
        assert_eq!(request.method, "GET");
        assert_eq!(request.headers["Authorization"], "Bearer validated-token");
        assert_eq!(request.headers["Accept"], "application/vnd.github+json");
        assert_eq!(request.headers["X-GitHub-Api-Version"], "2026-03-10");
        assert_eq!(request.headers["User-Agent"], "limitwatch");
    }
    assert_eq!(
        requests[0]
            .url
            .strip_prefix("https://api.github.com")
            .unwrap(),
        fixture["interstitial_paths"][0]
    );
    assert_eq!(
        requests[4]
            .url
            .strip_prefix("https://api.github.com")
            .unwrap(),
        fixture["interstitial_paths"][1]
    );
}

#[test]
fn github_billing_404_fallback() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/github_copilot/myriota_billing_404_fallback.json"
    ))
    .unwrap();
    let empty = || HttpResponse {
        status: 200,
        body: json!({"usageItems":[]}),
        headers: Default::default(),
    };
    let missing = || HttpResponse {
        status: 404,
        body: json!({"message":"Not Found"}),
        headers: Default::default(),
    };
    for all_missing in [false, true] {
        let mut responses = vec![empty()]; // /copilot_internal/user
        responses.extend(if all_missing {
            vec![missing(), missing(), missing(), missing()]
        } else {
            vec![missing(), empty(), missing(), empty()]
        });
        if !all_missing {
            responses.push(HttpResponse {
                status: 200,
                body: json!({"seat_breakdown":{"total":2},"plan_type":"business"}),
                headers: Default::default(),
            });
        }
        let http = Http {
            responses: Mutex::new(responses),
            requests: Mutex::new(vec![]),
        };
        let mut saved = account("github_copilot");
        saved.email = "Lucashutch".into();
        saved
            .extra
            .insert("githubToken".into(), json!("validated-token"));
        saved.extra.insert("organization".into(), json!("Myriota"));
        let mut provider = providers::create(saved).unwrap();
        let quotas =
            futures::executor::block_on(provider.fetch(&http, &Proc, &RequestContext::default()))
                .unwrap();
        let org = quotas.iter().find(|q| q.display_name == "Myriota").unwrap();
        assert_eq!(
            org.extra.get("is_error").and_then(Value::as_bool),
            all_missing.then_some(true)
        );
        if all_missing {
            assert!(org.extra["message"].as_str().unwrap().contains("HTTP 404"));
        }
        let requests = http.requests.lock().unwrap();
        let billing = requests
            .iter()
            .filter(|r| r.url.contains("/orgs/Myriota/settings/billing/usage"))
            .collect::<Vec<_>>();
        assert_eq!(billing.len(), 4);
        for (index, request) in billing.iter().enumerate() {
            let (path, query) = request
                .url
                .strip_prefix("https://api.github.com")
                .unwrap()
                .split_once('?')
                .unwrap();
            assert_eq!(path, fixture["billing_paths"][index]);
            let filtered = query.contains("product=copilot");
            assert_eq!(filtered, fixture["queries"][index] == "filtered");
        }
    }
}

#[test]
fn github_identity_validation_failure_is_not_treated_as_optional() {
    let http = Http {
        responses: Mutex::new(vec![HttpResponse {
            status: 403,
            body: json!({"message":"Forbidden"}),
            headers: Default::default(),
        }]),
        requests: Mutex::new(vec![]),
    };
    let mut provider = providers::create(account("github_copilot")).unwrap();
    let error = futures::executor::block_on(provider.login(
        json!({"githubToken":"identity-token", "organization":"Myriota"}),
        &http,
        &Proc,
        &RequestContext::default(),
    ))
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "GitHub token validation failed (HTTP 403): Forbidden Check token scopes, rate limits, and organization SSO authorization."
    );
}
