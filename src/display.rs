//! Terminal rendering parity for the quota view.
//!
//! Rich's tables, links, and terminal-width measurement do not have a direct
//! Rust equivalent here, so layout is intentionally fixed plain text with
//! small ANSI spans layered on top when stdout is a terminal.

use crate::{model::Quota, providers::base::Provider};
use chrono::{DateTime, TimeZone, Utc};

pub fn query_filter(mut quotas: Vec<Quota>, queries: &[String]) -> Vec<Quota> {
    for query in queries {
        let q = query.to_lowercase();
        quotas.retain(|x| {
            x.name.to_lowercase().contains(&q) || x.display_name.to_lowercase().contains(&q)
        });
    }
    quotas
}
fn pct(q: &Quota) -> (f64, bool) {
    q.used_pct
        .map(|value| (value, true))
        .unwrap_or((q.remaining_pct.unwrap_or(100.0), false))
}
fn quota_name(q: &Quota) -> &str {
    if q.display_name.is_empty() {
        &q.name
    } else {
        &q.display_name
    }
}
fn reset(q: &Quota, now: DateTime<Utc>) -> String {
    let Some(value) = q.reset_time.as_deref() else {
        return String::new();
    };
    let dt = DateTime::parse_from_rfc3339(&value.replace('Z', "+00:00"))
        .map(|x| x.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            value.parse::<f64>().ok().and_then(|mut x| {
                if x.abs() > 10_000_000_000.0 {
                    x /= 1000.0;
                }
                let seconds = x.trunc() as i64;
                let nanos = ((x - seconds as f64) * 1_000_000_000.0)
                    .round()
                    .clamp(0.0, 999_999_999.0) as u32;
                Utc.timestamp_opt(seconds, nanos).single()
            })
        });
    let Some(dt) = dt else {
        return String::new();
    };
    let seconds = (dt - now).num_seconds();
    if seconds <= 0 {
        return String::new();
    }
    format_countdown(seconds)
}
fn format_countdown(seconds: i64) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let (d, r) = (seconds / 86400, seconds % 86400);
    let (h, r) = (r / 3600, r % 3600);
    let m = r / 60;
    let mut p = Vec::new();
    if d > 0 {
        p.push(format!("{d}d"))
    }
    if h > 0 {
        p.push(format!("{h}h"))
    }
    if m > 0 || p.is_empty() {
        p.push(format!("{m}m"))
    }
    format!(" ({})", p.join(" "))
}
fn normal_bar_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(80)
        .saturating_sub(50)
        .clamp(10, 60)
}
fn bar(value: f64, width: usize, compact: bool) -> String {
    let value = value.clamp(0.0, 100.0);
    let n = (value * width as f64 / 100.0) as usize;
    let remainder = value * width as f64 / 100.0 - n as f64;
    let fractions = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let fraction = if compact || n >= width {
        ""
    } else {
        fractions[(remainder * 8.0) as usize]
    };
    format!(
        "{}{}{}",
        "█".repeat(n),
        fraction,
        " ".repeat(width - n - usize::from(!fraction.is_empty()))
    )
}
fn progress(raw: &str, color: &str, ansi: bool) -> String {
    let styled_width = raw.trim_end().chars().count();
    let split = raw
        .char_indices()
        .nth(styled_width)
        .map_or(raw.len(), |x| x.0);
    // Rich leaves the unfilled tail unstyled.  Keeping that tail plain is also
    // important for redirected output, where ANSI must never leak.
    format!(
        "{}{}",
        styled(&raw[..split], color_code(color), ansi),
        &raw[split..]
    )
}
fn color_code(color: &str) -> &'static str {
    match color {
        "red" => "31",
        "green" => "32",
        "yellow" => "33",
        "cyan" => "36",
        _ => "37",
    }
}
fn styled(value: &str, code: &str, color: bool) -> String {
    if color && !value.is_empty() {
        format!("\x1b[{code}m{value}\x1b[0m")
    } else {
        value.to_owned()
    }
}
pub fn color_enabled(is_terminal: bool, no_color: bool) -> bool {
    is_terminal && !no_color
}

pub fn main_header(color: bool) -> String {
    format!("\n{}\n", styled("Quota Status", "1;34", color))
}

pub fn separator(_color: bool) -> String {
    // Rich's heavy rule is deliberately rendered as plain text in the Rust
    // implementation; unlike Rich, it does not have a box/style abstraction.
    let value = "━".repeat(50);
    format!("{value}\n")
}

pub fn empty_message(quotas: &[Quota], show_all: bool) -> &'static str {
    if quotas.is_empty() || show_all {
        "No active quota information found."
    } else {
        "No premium models found (use --show-all to see all models)."
    }
}
pub fn account_header(
    email: &str,
    provider: &str,
    alias: Option<&str>,
    group: Option<&str>,
) -> String {
    let alias = alias.filter(|value| !value.is_empty());
    let group = group.filter(|value| !value.is_empty());
    let name = alias.unwrap_or(email);
    let meta = match (alias, group) {
        (Some(_), Some(g)) => format!(" ({email}|{g})"),
        (Some(_), None) => format!(" ({email})"),
        (None, Some(g)) => format!(" ({g})"),
        _ => String::new(),
    };
    let provider = if provider.is_empty() {
        String::new()
    } else {
        format!("{provider}: ")
    };
    format!("📧 {provider}{name}{meta}")
}

pub fn render_fetch_error(
    email: &str,
    alias: Option<&str>,
    group: Option<&str>,
    provider: Option<&dyn Provider>,
    error: &str,
    compact: bool,
    color: bool,
) -> String {
    let (provider_name, indicator, provider_color) = provider.map_or(("", '?', "37"), |p| {
        (
            p.provider_name(),
            p.short_indicator(),
            color_code(p.primary_color()),
        )
    });
    if compact {
        return format!(
            "{} {:10}: Warning: {}\n",
            styled(&indicator.to_string(), provider_color, color),
            alias
                .filter(|value| !value.is_empty())
                .unwrap_or(email)
                .chars()
                .take(10)
                .collect::<String>(),
            error
        );
    }
    format!(
        "{}\n{}\n{}",
        styled(
            &account_header(email, provider_name, alias, group),
            "2",
            color
        ),
        styled("Warning:", "33", color) + &format!(" {error}\n"),
        separator(color)
    )
}
pub fn render_quotas(
    email: &str,
    alias: Option<&str>,
    group: Option<&str>,
    provider: &dyn Provider,
    quotas: Vec<Quota>,
    compact: bool,
    color: bool,
) -> String {
    render_quotas_at(
        email,
        alias,
        group,
        provider,
        quotas,
        compact,
        color,
        Utc::now(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_quotas_at(
    email: &str,
    alias: Option<&str>,
    group: Option<&str>,
    provider: &dyn Provider,
    mut quotas: Vec<Quota>,
    compact: bool,
    color: bool,
    now: DateTime<Utc>,
) -> String {
    quotas.sort_by_key(|q| provider.sort_key(q));
    let mut out = format!(
        "{}\n",
        styled(
            &account_header(email, provider.provider_name(), alias, group),
            "2",
            color
        )
    );
    for q in quotas {
        let name = quota_name(&q);
        if q.extra.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
            let m = q
                .extra
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Validation Required");
            let url = q
                .extra
                .get("url")
                .or_else(|| q.extra.get("validation_url"))
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty());
            let link = url.map_or(String::new(), |_| " -> Click here to verify <-".into());
            let warning = styled(&format!("⚠️ {m}"), "31", color);
            let link = styled(&link, "2", color);
            if compact {
                let account = compact_account(email, alias);
                out += &format!(
                    "{} {:10}: {}: {}{}\n",
                    styled(
                        &provider.short_indicator().to_string(),
                        color_code(provider.primary_color()),
                        color
                    ),
                    account,
                    compact_name(name),
                    warning,
                    link
                );
            } else {
                out += &format!(
                    "{} {}{}\n",
                    styled(&format!("{name:22}"), color_code(provider.color(&q)), color),
                    warning,
                    link
                );
            }
            continue;
        }
        let usage_label = usage_label(&q);
        if q.extra.get("show_progress").and_then(|v| v.as_bool()) == Some(false) {
            let suffix = usage_label.map_or(String::new(), |x| format!(" {x}"));
            if compact {
                let account = compact_account(email, alias);
                out += &format!(
                    "{} {:10}: {}{}\n",
                    styled(
                        &provider.short_indicator().to_string(),
                        color_code(provider.primary_color()),
                        color
                    ),
                    account,
                    compact_name(name),
                    suffix
                );
            } else {
                out += &format!(
                    "{}{}\n",
                    styled(&format!("{name:22}"), color_code(provider.color(&q)), color),
                    suffix
                );
            }
            continue;
        }
        let (p, used) = pct(&q);
        let bar_color = if p <= 20.0 {
            if used {
                "green"
            } else {
                "red"
            }
        } else if p <= 50.0 {
            "yellow"
        } else if used {
            "red"
        } else {
            "green"
        };
        let percentage_text =
            usage_label.unwrap_or_else(|| format!("{p:5.1}%{}", if used { " used" } else { "" }));
        let percentage = styled(&percentage_text, color_code(bar_color), color);
        let countdown = styled(&reset(&q, now), "2", color);
        if compact {
            let account = compact_account(email, alias);
            let columns = std::env::var("COLUMNS")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(80);
            let prefix_width = 2 + 10 + 2;
            let bar_width = columns.saturating_sub(prefix_width + 30).clamp(5, 30);
            let raw = bar(p, bar_width, true);
            let progress = progress(&raw, bar_color, color);
            out += &format!(
                "{} {:10}: {:18} {} {}{}\n",
                styled(
                    &provider.short_indicator().to_string(),
                    color_code(provider.primary_color()),
                    color
                ),
                account,
                compact_name(name).chars().take(18).collect::<String>(),
                progress,
                percentage,
                countdown
            );
        } else {
            out += &format!(
                "{} {} {}{}\n",
                styled(&format!("{name:22}"), color_code(provider.color(&q)), color),
                progress(&bar(p, normal_bar_width(), false), bar_color, color),
                percentage,
                countdown
            );
        }
    }
    out
}

fn compact_name(name: &str) -> String {
    let name = name.strip_prefix("Gemini ").unwrap_or(name);
    let name = if let Some((value, suffix)) = name
        .strip_suffix(")")
        .and_then(|value| value.rsplit_once(" ("))
    {
        let Some((left, right)) = suffix.split_once('/') else {
            return name.chars().take(18).collect();
        };
        if left.chars().all(|x| x.is_ascii_digit()) && right.chars().all(|x| x.is_ascii_digit()) {
            value
        } else {
            name
        }
    } else {
        name
    };
    name.to_owned()
}

fn compact_account(email: &str, alias: Option<&str>) -> String {
    let account = alias.filter(|value| !value.is_empty()).unwrap_or(email);
    let account = account.split_once(": ").map_or(account, |(_, value)| value);
    account.chars().take(10).collect()
}

fn usage_label(q: &Quota) -> Option<String> {
    if let Some(label) = q
        .extra
        .get("usage_label")
        .and_then(|value| value.as_str())
        .filter(|label| !label.is_empty())
    {
        return Some(label.to_owned());
    }
    if q.extra
        .get("billing_model")
        .and_then(|value| value.as_str())
        == Some("ai_credits")
    {
        let used = q.used.unwrap_or(0.0);
        let value = format_number(used);
        if q.extra
            .get("show_progress")
            .and_then(|value| value.as_bool())
            == Some(false)
        {
            return Some(format!("{value} cr"));
        }
        return Some(format!("{value} cr ({:.1}%)", q.used_pct.unwrap_or(0.0)));
    }
    None
}

fn format_number(value: f64) -> String {
    let raw = if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let sign = if whole.starts_with('-') { "-" } else { "" };
    let whole = whole.trim_start_matches('-');
    let grouped = whole
        .chars()
        .rev()
        .enumerate()
        .fold(String::new(), |mut out, (index, ch)| {
            if index > 0 && index % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
            out
        })
        .chars()
        .rev()
        .collect::<String>();
    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Account, providers};
    use serde_json::json;

    fn provider(kind: &str) -> Box<dyn Provider> {
        providers::create(Account {
            provider_type: kind.into(),
            email: "octo".into(),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn standard_output_matches_used_percent_and_copilot_org_rows() {
        let openai = Quota {
            display_name: "Five hour".into(),
            used_pct: Some(25.0),
            ..Default::default()
        };
        let text = render_quotas(
            "me@example.com",
            None,
            None,
            &*provider("openai"),
            vec![openai],
            false,
            false,
        );
        assert!(text.contains("📧 OpenAI Codex: me@example.com\n"));
        assert!(text.contains(" 25.0% used\n"));

        let org = Quota {
            display_name: "acme".into(),
            remaining_pct: Some(80.0),
            ..Default::default()
        };
        let text = render_quotas(
            "octo",
            None,
            None,
            &*provider("github_copilot"),
            vec![org],
            false,
            false,
        );
        assert!(text.contains("📧 GitHub Copilot: octo\n"));
        assert!(text.contains("acme"));
        assert!(text.contains(" 80.0%"));
    }

    #[test]
    fn ansi_only_decorates_bar_and_errors_have_no_separator_or_countdown() {
        let quota = Quota {
            display_name: "Personal".into(),
            remaining_pct: Some(50.0),
            ..Default::default()
        };
        let plain = render_quotas(
            "octo",
            None,
            None,
            &*provider("github_copilot"),
            vec![quota.clone()],
            false,
            false,
        );
        let ansi = render_quotas(
            "octo",
            None,
            None,
            &*provider("github_copilot"),
            vec![quota],
            false,
            true,
        );
        let mut stripped = ansi;
        for code in ["\x1b[0m", "\x1b[2m", "\x1b[32m", "\x1b[33m", "\x1b[37m"] {
            stripped = stripped.replace(code, "");
        }
        assert_eq!(stripped, plain);

        let mut error = Quota {
            display_name: "acme".into(),
            ..Default::default()
        };
        error.extra.insert("is_error".into(), json!(true));
        error
            .extra
            .insert("message".into(), json!("billing unavailable"));
        assert_eq!(
            render_quotas(
                "octo",
                None,
                None,
                &*provider("github_copilot"),
                vec![error],
                false,
                false
            ),
            "📧 GitHub Copilot: octo\nacme                   ⚠️ billing unavailable\n"
        );
    }

    #[test]
    fn countdown_uses_compact_units_without_absolute_time() {
        assert_eq!(format_countdown(19 * 60), " (19m)");
        assert_eq!(
            format_countdown(5 * 86_400 + 19 * 3_600 + 43 * 60),
            " (5d 19h 43m)"
        );
    }

    #[test]
    fn normal_bar_fraction_and_standard_row_have_stable_spacing() {
        assert_eq!(bar(25.5, 30, false), "███████▋                      ");
        let quota = Quota {
            display_name: "Five hour".into(),
            used_pct: Some(25.5),
            ..Default::default()
        };
        let text = render_quotas(
            "me@example.com",
            None,
            None,
            &*provider("openai"),
            vec![quota],
            false,
            false,
        );
        assert_eq!(
            text,
            "📧 OpenAI Codex: me@example.com\nFive hour              ███████▋                        25.5% used\n"
        );
    }

    #[test]
    fn normal_color_and_plain_goldens() {
        std::env::set_var("COLUMNS", "80");
        let quotas = vec![
            Quota {
                display_name: "Remaining".into(),
                remaining_pct: Some(75.0),
                ..Default::default()
            },
            Quota {
                display_name: "Used".into(),
                used_pct: Some(25.0),
                ..Default::default()
            },
        ];
        let plain = render_quotas(
            "me@example.com",
            Some("work"),
            Some("team"),
            &*provider("openai"),
            quotas.clone(),
            false,
            false,
        );
        let ansi = render_quotas(
            "me@example.com",
            Some("work"),
            Some("team"),
            &*provider("openai"),
            quotas,
            false,
            true,
        );
        assert_eq!(
            plain,
            include_str!("../tests/fixtures/golden/normal_quota_plain.txt")
        );
        assert_eq!(
            ansi,
            include_str!("../tests/fixtures/golden/normal_quota_ansi.txt")
        );
    }

    #[test]
    fn compact_and_metadata_rows_use_fixed_clock_and_expected_labels() {
        std::env::set_var("COLUMNS", "80");
        let mut credits = Quota {
            display_name: "Premium".into(),
            used: Some(1234.5),
            used_pct: Some(12.345),
            remaining_pct: Some(87.655),
            reset_time: Some("2026-07-13T00:00:00Z".into()),
            ..Default::default()
        };
        credits
            .extra
            .insert("billing_model".into(), json!("ai_credits"));
        let mut balance = Quota {
            display_name: "Balance".into(),
            ..Default::default()
        };
        balance.extra.insert("show_progress".into(), json!(false));
        balance
            .extra
            .insert("usage_label".into(), json!("$2.50 remaining"));
        let text = render_quotas_at(
            "me@example.com",
            Some("work"),
            Some("team"),
            &*provider("openai"),
            vec![credits, balance],
            true,
            false,
            Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap(),
        );
        assert!(text.contains("O work      : Premium"), "{text}");
        assert!(text.contains("1,234.5 cr (12.3%) (1d)"), "{text}");
        assert!(
            text.contains("O work      : Balance $2.50 remaining"),
            "{text}"
        );
    }

    #[test]
    fn countdown_is_shown_for_an_unused_quota() {
        let quota = Quota {
            display_name: "Five hour".into(),
            used_pct: Some(0.0),
            remaining_pct: Some(100.0),
            reset_time: Some("2026-07-13T05:00:00Z".into()),
            ..Default::default()
        };
        let text = render_quotas_at(
            "me@example.com",
            None,
            None,
            &*provider("openai"),
            vec![quota],
            false,
            false,
            Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap(),
        );

        assert!(text.contains("Five hour              "), "{text}");
        assert!(text.contains(" (1d 5h)"), "{text}");
    }

    #[test]
    fn text_only_and_ai_credit_rows_keep_labels_in_both_layouts() {
        let mut credits = Quota {
            display_name: "AI Credits".into(),
            used: Some(231.3),
            used_pct: Some(0.9),
            remaining_pct: Some(99.1),
            ..Default::default()
        };
        credits
            .extra
            .insert("billing_model".into(), json!("ai_credits"));
        let mut balance = Quota {
            display_name: "Balance".into(),
            ..Default::default()
        };
        balance.extra.insert("show_progress".into(), json!(false));
        balance
            .extra
            .insert("usage_label".into(), json!("$2.50 remaining"));

        let normal = render_quotas(
            "octo",
            Some(""),
            Some(""),
            &*provider("github_copilot"),
            vec![credits.clone(), balance.clone()],
            false,
            false,
        );
        assert!(normal.starts_with("📧 GitHub Copilot: octo\n"));
        assert!(normal.contains("AI Credits             "));
        assert!(normal.contains("231.3 cr (0.9%)"));
        assert!(normal.contains("Balance                $2.50 remaining\n"));

        let compact = render_quotas(
            "octo",
            Some(""),
            Some(""),
            &*provider("github_copilot"),
            vec![credits, balance],
            true,
            false,
        );
        assert!(compact.contains("H octo      : AI Credits"));
        assert!(compact.contains("231.3 cr (0.9%)"));
        assert!(compact.contains("H octo      : Balance $2.50 remaining\n"));
    }

    #[test]
    fn empty_messages_and_validation_links_match_plain_rich_text() {
        assert_eq!(
            empty_message(&[], false),
            "No active quota information found."
        );
        assert_eq!(
            empty_message(&[Quota::default()], false),
            "No premium models found (use --show-all to see all models)."
        );
        let mut error = Quota {
            display_name: "Account".into(),
            ..Default::default()
        };
        error.extra.insert("is_error".into(), json!(true));
        error
            .extra
            .insert("message".into(), json!("Verify your account to continue."));
        error
            .extra
            .insert("url".into(), json!("https://example.test"));
        let text = render_quotas(
            "octo",
            None,
            None,
            &*provider("github_copilot"),
            vec![error],
            false,
            false,
        );
        assert!(text.contains("⚠️ Verify your account to continue. -> Click here to verify <-"));
        assert!(!text.contains("https://example.test"));
    }
}
