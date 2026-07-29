use crate::{
    auth::AuthManager,
    completions,
    config::Config,
    display,
    export::{ExportFilter, Exporter},
    history::HistoryManager,
    model::{Account, Quota, Timing},
    providers,
    providers::base::{HttpClient, ProcessRunner, RequestContext},
    quota_client::QuotaClient,
};
use anyhow::{bail, Result};
use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const VERSION: &str = env!("VERGEN_GIT_DESCRIBE");

#[derive(Parser)]
#[command(
    name = "limitwatch",
    version = VERSION,
    disable_version_flag = true,
    about = "Monitor API quota usage and reset times across all accounts",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[arg(
        short = 'v',
        long = "version",
        action = ArgAction::SetTrue
    )]
    version: bool,
    #[command(flatten)]
    show: ShowArgs,
    #[command(subcommand)]
    command: Option<Commands>,
}
#[derive(Subcommand)]
enum Commands {
    Show(ShowArgs),
    History(HistoryArgs),
    Export(ExportArgs),
    Completion {
        #[arg(value_parser=["bash","zsh","fish"])]
        shell: String,
    },
    #[command(hide = true)]
    Complete {
        kind: String,
        prefix: String,
    },
}
#[derive(Args, Clone, Default)]
pub struct ShowArgs {
    #[arg(short, long, help = "Email of the account to check; may be repeated")]
    account: Vec<String>,
    #[arg(long, help = "Set or clear an alias for one account")]
    alias: Option<String>,
    #[arg(short, long, help = "Filter by group, or set a group with --account")]
    group: Option<String>,
    #[arg(short, long, help = "Filter by provider; may be repeated")]
    provider: Vec<String>,
    #[arg(short, long, help = "Filter quotas by name; may be repeated")]
    query: Vec<String>,
    #[arg(short, long)]
    refresh: bool,
    #[arg(short = 's', long)]
    show_all: bool,
    #[arg(short, long)]
    compact: bool,
    #[arg(short = 'j', long = "json")]
    json_output: bool,
    #[arg(short, long)]
    login: bool,
    #[arg(long)]
    project_id: Option<String>,
    #[arg(long)]
    logout: bool,
    #[arg(long)]
    logout_all: bool,
    /// Make the identified supported account active (Rust extension)
    #[arg(long)]
    select_account: Option<String>,
    #[arg(long)]
    no_record: bool,
    #[arg(long)]
    verbose: bool,
    #[arg(long)]
    timings: bool,
    #[arg(long, default_value_t = 4000)]
    max_age_ms: u64,
    #[arg(long)]
    cache_ttl: Option<u64>,
}
#[derive(Args)]
struct HistoryArgs {
    #[arg(long,value_parser=["24h","7d","30d","90d"])]
    preset: Option<String>,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    until: Option<String>,
    #[arg(short, long)]
    account: Option<String>,
    #[arg(short, long)]
    provider: Option<String>,
    #[arg(short = 'q', long)]
    quota: Option<String>,
    #[arg(long)]
    table: bool,
    #[arg(long)]
    summary: bool,
    #[arg(long)]
    heatmap: bool,
    #[arg(long)]
    chart: bool,
    #[arg(long)]
    calendar: bool,
    #[arg(long)]
    bars: bool,
    #[arg(long)]
    stats: bool,
    #[arg(long)]
    verbose: bool,
}
#[derive(Args)]
struct ExportArgs {
    #[arg(long, value_enum, default_value = "csv")]
    format: Format,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long,value_parser=["24h","7d","30d","90d"])]
    preset: Option<String>,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    until: Option<String>,
    #[arg(short, long)]
    account: Option<String>,
    #[arg(short, long)]
    provider: Option<String>,
    #[arg(short = 'q', long)]
    quota: Option<String>,
    #[arg(long)]
    verbose: bool,
}
#[derive(Clone, ValueEnum)]
enum Format {
    Csv,
    Markdown,
}
struct Proc;
impl ProcessRunner for Proc {
    fn run(
        &self,
        p: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<crate::providers::base::ProcessOutput> {
        let mut child = Command::new(p)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let started = Instant::now();
        loop {
            if child.try_wait()?.is_some() {
                let o = child.wait_with_output()?;
                return Ok(crate::providers::base::ProcessOutput {
                    success: o.status.success(),
                    stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
                });
            }
            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                bail!("process timed out");
            }
            thread::sleep(Duration::from_millis(5).min(timeout.saturating_sub(started.elapsed())));
        }
    }
}
#[derive(Serialize)]
struct JsonResult {
    email: String,
    alias: String,
    group: String,
    quotas: Option<Vec<Quota>>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timings: Option<Vec<Timing>>,
}
struct Fetch {
    index: usize,
    account: Account,
    quotas: Vec<Quota>,
    error: Option<String>,
    timings: Vec<Timing>,
    client: Option<QuotaClient>,
}
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.version {
        println!(
            "limitwatch {}",
            VERSION.strip_prefix('v').unwrap_or(VERSION)
        );
        return Ok(());
    }
    match cli.command {
        None => show(cli.show),
        Some(Commands::Show(x)) => show(x),
        Some(Commands::History(x)) => history(x),
        Some(Commands::Export(x)) => export(x),
        Some(Commands::Completion { shell }) => {
            completions::generate_script(&shell, Cli::command())
        }
        Some(Commands::Complete { kind, prefix }) => {
            for x in completions::candidates(&kind, &prefix) {
                println!("{x}")
            }
            Ok(())
        }
    }
}
fn show(a: ShowArgs) -> Result<()> {
    let config = Config::new(None);
    let mut auth = AuthManager::new(config.auth_path());
    if a.login {
        return login(&mut auth, &a);
    }
    if let Some(id) = &a.select_account {
        let matches = auth
            .supported_accounts()
            .filter(|(_, x)| x.email == *id || x.identity() == id || x.alias.as_deref() == Some(id))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return status(&a, "error", "Account not found or ambiguous");
        }
        auth.active_index = matches[0];
        auth.save_accounts()?;
        return status(&a, "success", &format!("Selected account {id}"));
    }
    if a.logout {
        if let Some(id) = a.account.first() {
            if a.json_output || !io::stdin().is_terminal() {
                return status(
                    &a,
                    "error",
                    "--logout requires interactive confirmation in non-interactive mode",
                );
            }
            let matches = auth
                .supported_accounts()
                .filter(|(_, account)| {
                    account.email == *id
                        || account.identity() == id
                        || account.alias.as_deref() == Some(id)
                })
                .map(|(_, account)| account.clone())
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return status(&a, "error", "Account not found or ambiguous");
            }
            let label = matches[0]
                .alias
                .as_deref()
                .unwrap_or(&matches[0].email)
                .to_owned();
            if !matches!(
                prompt(&format!("Log out {label}? [y/N]: "))?
                    .to_ascii_lowercase()
                    .as_str(),
                "y" | "yes"
            ) {
                println!("Logout cancelled.");
                return Ok(());
            }
            if !auth.logout(id)? {
                return status(&a, "error", "Account not found or ambiguous");
            }
            return status(&a, "success", &format!("Successfully logged out {label}"));
        }
        if a.json_output || !io::stdin().is_terminal() {
            return status(
                &a,
                "error",
                "--logout requires --account in non-interactive mode",
            );
        }
        return interactive_logout(&mut auth);
    }
    if a.logout_all {
        return logout_all(&mut auth, &a);
    }
    for provider in &a.provider {
        if !providers::available()
            .iter()
            .any(|(name, _)| name == provider)
        {
            return status(&a, "error", &format!("unsupported provider: {provider}"));
        }
    }
    if !auth.auth_path.exists() {
        if is_interactive(&a) {
            let answer = prompt("Accounts file not found. Would you like to log in? [Y/n]: ")?;
            if login_requested(&answer) {
                return login(&mut auth, &a);
            }
        }
        return status(&a, "error", "Accounts file not found");
    }
    if auth.accounts.is_empty() {
        return status(&a, "error", "No accounts found");
    }
    if (a.alias.is_some() || a.group.is_some() || a.project_id.is_some()) && !a.account.is_empty() {
        if a.account.len() != 1 {
            return status(&a, "error", "Metadata update requires a single account");
        }
        let matches = auth
            .accounts
            .iter()
            .filter(|x| {
                x.is_supported()
                    && (x.email == a.account[0]
                        || x.identity() == a.account[0]
                        || x.alias.as_deref() == Some(&a.account[0]))
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return status(&a, "error", "Account not found or ambiguous");
        }
        let email = matches[0].email.clone();
        let mut m = BTreeMap::new();
        if a.alias.is_some() {
            m.insert("alias".into(), a.alias.clone());
        }
        if a.group.is_some() {
            m.insert("group".into(), a.group.clone());
        }
        if let Some(p) = &a.project_id {
            m.insert("projectId".into(), Some(p.clone()));
            m.insert("managedProjectId".into(), Some(p.clone()));
        }
        if !auth.update_account_metadata(&email, &m)? {
            return status(&a, "error", "Account not found or ambiguous");
        }
        return status(&a, "success", &format!("Updated metadata for {email}"));
    }
    let selected = auth
        .supported_accounts()
        .map(|(i, x)| (i, x.clone()))
        .filter(|(_, x)| {
            (a.account.is_empty()
                || a.account
                    .iter()
                    .any(|v| v == &x.email || Some(v.as_str()) == x.alias.as_deref()))
                && (a.provider.is_empty() || a.provider.contains(&x.provider_type))
                && (a.group.is_none() || !a.account.is_empty() || x.group == a.group)
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return status(&a, "error", "No accounts matching filters");
    }
    let show_start = Instant::now();
    let deadline = show_start + Duration::from_millis(a.max_age_ms);
    let force_refresh = a.refresh;
    let http = crate::quota_client::SharedHttp::new()?;
    let (tx, rx) = mpsc::channel();
    // Provider calls are bounded independently of the number of accounts.
    // Each request still receives the same absolute deadline below.
    let jobs = Arc::new(Mutex::new(VecDeque::from(selected.clone())));
    for _ in 0..selected.len().min(10) {
        let tx = tx.clone();
        let http = http.clone();
        let jobs = Arc::clone(&jobs);
        thread::spawn(move || {
            loop {
                let Some((index, mut account)) =
                    jobs.lock().expect("fetch job lock poisoned").pop_front()
                else {
                    break;
                };
                if force_refresh {
                    account
                        .extra
                        .insert("_limitwatch_force_refresh".into(), Value::Bool(true));
                }
                let original = account.clone();
                let mut client = match QuotaClient::new(account) {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = tx.send(Fetch {
                            index,
                            account: original,
                            quotas: vec![],
                            error: Some(e.to_string()),
                            timings: vec![],
                            client: None,
                        });
                        continue;
                    }
                };
                let start = Instant::now();
                let account_http = http.clone();
                let ctx = RequestContext {
                    deadline: Some(deadline),
                    ..Default::default()
                };
                let result = futures::executor::block_on(client.fetch(&account_http, &Proc, &ctx));
                let mut timings = vec![Timing {
                    name: "account_total".into(),
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    extra: BTreeMap::new(),
                }];
                timings.extend(client.provider().timings());
                timings.extend(account_http.timings());
                let (q, e) = match result {
                    Ok(q) => (q, None),
                    Err(e) => (vec![], Some(e.to_string())),
                };
                // Providers may rotate credentials while fetching.  Return
                // their account state so it can be persisted by the caller.
                let account = client.account().clone();
                let _ = tx.send(Fetch {
                    index,
                    account,
                    quotas: q,
                    error: e,
                    timings,
                    client: Some(client),
                });
            }
        });
    }
    drop(tx);
    let mut fetched = Vec::new();
    while fetched.len() < selected.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(x) => fetched.push(finalize_fetch(
                x,
                a.cache_ttl.unwrap_or(config.cache_ttl()),
                a.max_age_ms,
                show_start,
            )),
            Err(_) => break,
        }
    }
    for (index, account) in selected {
        if !fetched.iter().any(|x| x.index == index) {
            let elapsed_ms = show_start.elapsed().as_secs_f64() * 1000.0;
            let (quotas, error, timing_name, timing_reason) =
                match cached(&account, a.cache_ttl.unwrap_or(config.cache_ttl())) {
                    Some(quotas) => (quotas, None, "cache_fallback", "timeout_cache"),
                    None => (
                        vec![],
                        Some("Timed out (no cached data available)".into()),
                        "deadline_missed",
                        "timeout_no_cache",
                    ),
                };
            fetched.push(Fetch {
                index,
                account: account.clone(),
                quotas,
                error,
                timings: vec![
                    Timing {
                        name: timing_name.into(),
                        elapsed_ms: 0.0,
                        extra: [("reason".into(), Value::String(timing_reason.into()))]
                            .into_iter()
                            .collect(),
                    },
                    Timing {
                        name: "account_total".into(),
                        elapsed_ms,
                        extra: BTreeMap::new(),
                    },
                ],
                client: QuotaClient::new(account).ok(),
            });
        }
    }
    fetched.sort_by_key(|x| x.index);
    if a.verbose {
        for f in &fetched {
            eprintln!(
                "provider={} quotas={} status={} elapsed_ms={:.1}",
                f.account.provider_type,
                f.quotas.len(),
                if f.error.is_some() { "error" } else { "ok" },
                f.timings.first().map(|t| t.elapsed_ms).unwrap_or(0.0)
            );
            for line in github_verbose(a.verbose, &f.timings) {
                eprintln!("{line}");
            }
        }
    }
    let mut cache_changed = false;
    for f in &mut fetched {
        if let Some(client) = &f.client {
            auth.accounts[f.index] = client.account().clone();
            auth.accounts[f.index]
                .extra
                .remove("_limitwatch_force_refresh");
            cache_changed = true;
        }
        let used_cache = f
            .timings
            .iter()
            .any(|timing| timing.name == "cache_fallback");
        if f.error.is_none() && !used_cache && should_cache(&f.quotas) {
            auth.accounts[f.index]
                .extra
                .insert("cachedQuotas".into(), serde_json::to_value(&f.quotas)?);
            auth.accounts[f.index]
                .extra
                .insert("cachedAt".into(), json!(now()));
            cache_changed = true;
        }
    }
    if cache_changed {
        auth.save_accounts()?;
    }
    if !a.no_record && config.history_enabled() {
        let h = HistoryManager::new(Some(config.history_db_path()))?;
        for f in &fetched {
            if f.error.is_none() && !f.quotas.is_empty() {
                let _ =
                    h.record_quotas(&f.account.email, &f.account.provider_type, &f.quotas, None);
            }
        }
    }
    let mut matched = false;
    if a.json_output {
        let mut out = Vec::new();
        for f in fetched {
            let quotas = if f.error.is_some() {
                None
            } else {
                let mut quotas = f.quotas;
                if let Some(c) = &f.client {
                    quotas = c.filter(quotas, a.show_all)
                }
                quotas = display::query_filter(quotas, &a.query);
                matched |= !quotas.is_empty();
                Some(quotas)
            };
            out.push(JsonResult {
                email: f.account.email,
                alias: f.account.alias.unwrap_or_default(),
                group: f.account.group.unwrap_or_default(),
                quotas,
                error: f.error,
                timings: a.timings.then_some(f.timings),
            });
        }
        println!("{}", serde_json::to_string_pretty(&out)?)
    } else {
        let color = display::color_enabled(
            io::stdout().is_terminal(),
            std::env::var_os("NO_COLOR").is_some(),
        );
        if a.query.is_empty() && !a.compact {
            print!("{}", display::main_header(color));
        }
        for mut f in fetched {
            if let Some(e) = f.error.as_deref() {
                // An account-level error excluded by --query is silent; it
                // must not make an unrelated query visibly fail.
                if !a.query.is_empty() {
                    continue;
                }
                print!(
                    "{}",
                    display::render_fetch_error(
                        &f.account.email,
                        f.account.alias.as_deref(),
                        f.account.group.as_deref(),
                        f.client.as_ref().map(|x| x.provider()),
                        e,
                        a.compact,
                        color,
                    )
                );
                continue;
            }
            let Some(c) = f.client else { continue };
            let original_quotas = f.quotas.clone();
            f.quotas = c.filter(f.quotas, a.show_all);
            f.quotas = display::query_filter(f.quotas, &a.query);
            matched |= !f.quotas.is_empty();
            if f.quotas.is_empty() && !a.query.is_empty() {
                continue;
            }
            if f.quotas.is_empty() {
                print!(
                    "{}",
                    display::render_quotas(
                        &f.account.email,
                        f.account.alias.as_deref(),
                        f.account.group.as_deref(),
                        c.provider(),
                        vec![],
                        a.compact,
                        color,
                    )
                );
                println!("{}", display::empty_message(&original_quotas, a.show_all));
            } else {
                print!(
                    "{}",
                    display::render_quotas(
                        &f.account.email,
                        f.account.alias.as_deref(),
                        f.account.group.as_deref(),
                        c.provider(),
                        f.quotas,
                        a.compact,
                        color,
                    )
                );
            }
            if !a.compact {
                print!("{}", display::separator(color));
            }
        }
    }
    if !a.query.is_empty() && !matched {
        bail!("No quotas matched query")
    }
    Ok(())
}

/// Render the deliberately narrow GitHub diagnostic contract. Unknown trace
/// fields are ignored so credentials and response data can never be emitted.
pub fn github_verbose(verbose: bool, timings: &[crate::model::Timing]) -> Vec<String> {
    if !verbose {
        return vec![];
    }
    timings
        .iter()
        .filter_map(|trace| match trace.name.as_str() {
            "github_request" => Some(format!(
                "[GitHub] {} {} status={}",
                trace.extra.get("method")?,
                trace.extra.get("path")?,
                trace.extra.get("status")?
            )),
            "github_selection" => trace
                .extra
                .get("outcome")
                .map(|outcome| format!("[GitHub] {outcome}")),
            _ => None,
        })
        .collect()
}

fn interactive_logout(auth: &mut AuthManager) -> Result<()> {
    let accounts = auth
        .supported_accounts()
        .map(|(_, a)| a.clone())
        .collect::<Vec<_>>();
    if accounts.is_empty() {
        println!("No accounts found to log out from.");
        return Ok(());
    }
    let mut providers = Vec::new();
    for provider in ["github_copilot", "openai", "openrouter"] {
        if accounts.iter().any(|a| a.provider_type == provider) {
            providers.push(provider);
        }
    }
    println!("Select Provider to log out from:");
    for (index, provider) in providers.iter().enumerate() {
        let count = accounts
            .iter()
            .filter(|a| a.provider_type == *provider)
            .count();
        let label = match *provider {
            "github_copilot" => "GitHub Copilot",
            "openai" => "OpenAI",
            "openrouter" => "OpenRouter",
            _ => provider,
        };
        println!(
            "{}) {label} ({count} account{})",
            index + 1,
            if count == 1 { "" } else { "s" }
        );
    }
    let provider_choice = prompt("Enter choice [1]: ")?;
    let index = choice_index(&provider_choice, providers.len());
    let Some(provider) = index.and_then(|i| providers.get(i)) else {
        println!("Invalid choice.");
        return Ok(());
    };
    let provider_accounts = accounts
        .iter()
        .filter(|a| a.provider_type == *provider)
        .collect::<Vec<_>>();
    let account = if provider_accounts.len() == 1 {
        provider_accounts[0]
    } else {
        println!("Select Account to log out:");
        for (index, account) in provider_accounts.iter().enumerate() {
            println!(
                "{}) {}",
                index + 1,
                account.alias.as_deref().unwrap_or(&account.email)
            );
        }
        let account_choice = prompt("Enter choice [1]: ")?;
        let Some(account) = choice_index(&account_choice, provider_accounts.len())
            .and_then(|i| provider_accounts.get(i))
        else {
            println!("Invalid choice.");
            return Ok(());
        };
        account
    };
    let label = account
        .alias
        .as_deref()
        .unwrap_or(&account.email)
        .to_owned();
    let answer = prompt(&format!("Log out {label}? [y/N]: "))?;
    if !matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("Logout cancelled.");
        return Ok(());
    }
    auth.logout(&account.email)?;
    println!("Successfully logged out {label}");
    Ok(())
}

fn choice_index(value: &str, len: usize) -> Option<usize> {
    let choice = if value.is_empty() {
        1
    } else {
        value.parse().ok()?
    };
    (choice > 0 && choice <= len).then_some(choice - 1)
}

fn logout_all(auth: &mut AuthManager, a: &ShowArgs) -> Result<()> {
    let count = auth.supported_accounts().count();
    if !a.json_output {
        if count == 0 {
            println!("No accounts to log out from.");
            return Ok(());
        }
        let answer = prompt(&format!(
            "Log out from all {count} account{}? [y/N]: ",
            if count == 1 { "" } else { "s" }
        ))?;
        if !matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Logout cancelled.");
            return Ok(());
        }
    }
    auth.logout_all()?;
    if a.json_output {
        println!("{}", json!({"status": "success"}));
    } else {
        println!("Successfully logged out from all accounts.");
    }
    Ok(())
}
fn cached(a: &Account, ttl: u64) -> Option<Vec<Quota>> {
    let at = a
        .extra
        .get("cachedAt")
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(0.0);
    if ttl == 0 || now() - at > ttl as f64 {
        return None;
    }
    a.extra
        .get("cachedQuotas")
        .cloned()
        .and_then(|x| serde_json::from_value(x).ok())
}

fn should_cache(quotas: &[Quota]) -> bool {
    !quotas.is_empty()
        && quotas
            .iter()
            .any(|quota| quota.extra.get("is_error").and_then(Value::as_bool) != Some(true))
}

fn finalize_fetch(mut fetch: Fetch, ttl: u64, max_age_ms: u64, show_start: Instant) -> Fetch {
    if show_start.elapsed().as_secs_f64() * 1000.0 <= max_age_ms as f64 {
        return fetch;
    }
    let had_error = fetch.error.is_some();
    if let Some(quotas) = cached(&fetch.account, ttl) {
        fetch.quotas = quotas;
        fetch.error = None;
        fetch.timings.push(Timing {
            name: "cache_fallback".into(),
            elapsed_ms: 0.0,
            extra: [(
                "reason".into(),
                Value::String(
                    if had_error {
                        "error_cache"
                    } else {
                        "timeout_cache"
                    }
                    .into(),
                ),
            )]
            .into_iter()
            .collect(),
        });
    } else if fetch.error.is_none() {
        fetch.quotas.clear();
        fetch.error = Some("Timed out (no cached data available)".into());
        fetch.timings.push(Timing {
            name: "deadline_missed".into(),
            elapsed_ms: 0.0,
            extra: [("reason".into(), Value::String("timeout_no_cache".into()))]
                .into_iter()
                .collect(),
        });
    }
    fetch
}
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn status(a: &ShowArgs, status: &str, message: &str) -> Result<()> {
    if a.json_output {
        println!("{}", json!({"status":status,"message":message}))
    } else {
        println!("{message}")
    }
    Ok(())
}
fn action_error(a: &ShowArgs, message: impl std::fmt::Display) -> Result<()> {
    if a.json_output {
        println!(
            "{}",
            json!({"status": "error", "message": message.to_string()})
        );
    } else {
        println!("Login failed: {message}");
    }
    Ok(())
}
fn login(auth: &mut AuthManager, a: &ShowArgs) -> Result<()> {
    let p = if let Some(p) = a.provider.first() {
        p.as_str()
    } else if is_interactive(a) {
        eprint!("Select provider [1] GitHub Copilot [2] OpenAI [3] OpenRouter (default 1): ");
        io::stderr().flush()?;
        let mut value = String::new();
        io::stdin().read_line(&mut value)?;
        match value.trim() {
            "2" | "openai" => "openai",
            "3" | "openrouter" => "openrouter",
            _ => "github_copilot",
        }
    } else {
        "github_copilot"
    };
    if !crate::providers::available()
        .iter()
        .any(|(name, _)| *name == p)
    {
        return status(a, "error", &format!("unsupported provider: {p}"));
    }
    let mut client = match QuotaClient::new(Account {
        provider_type: p.into(),
        email: "pending".into(),
        ..Default::default()
    }) {
        Ok(client) => client,
        Err(error) => return action_error(a, error),
    };
    let login_json = std::env::var("LIMITWATCH_LOGIN_JSON").ok();
    let mut input = login_json
        .as_deref()
        .and_then(|x| serde_json::from_str(x).ok())
        .unwrap_or(Value::Null);
    if p == "github_copilot" && input.is_null() {
        let http = match crate::quota_client::SharedHttp::new() {
            Ok(http) => http,
            Err(error) => return action_error(a, error),
        };
        input = match github_login_input(&http, &Proc, &RequestContext::default()) {
            Ok(input) => input,
            Err(error) => return action_error(a, error),
        };
    }
    let interactive = is_interactive(a);
    if p == "openrouter" {
        input = openrouter_login_input(input, interactive && login_json.is_none(), || {
            prompt("Enter OpenRouter API key: ")
        })?;
    }
    let http = match crate::quota_client::SharedHttp::new() {
        Ok(http) => http,
        Err(error) => return action_error(a, error),
    };
    let mut account = match futures::executor::block_on(client.login(
        input,
        &http,
        &Proc,
        &RequestContext::default(),
    )) {
        Ok(account) => account,
        Err(error) => return action_error(a, error),
    };
    if p == "openrouter"
        && account
            .extra
            .remove("_limitwatch_openrouter_needs_name")
            .and_then(|value| value.as_bool())
            == Some(true)
        && interactive
    {
        let name = prompt("Key validated. Enter a friendly name for this account (optional): ")?;
        if !name.is_empty() {
            account.email = name;
        }
    }
    let email = match auth.login(account) {
        Ok(email) => email,
        Err(error) => return action_error(a, error),
    };
    if a.json_output {
        println!("{}", json!({"status": "success", "email": email}));
    } else {
        println!("Successfully logged in as {email}");
    }
    Ok(())
}
fn openrouter_login_input<F>(input: Value, interactive: bool, prompt_for_key: F) -> Result<Value>
where
    F: FnOnce() -> Result<String>,
{
    if !input.is_null() || !interactive {
        return Ok(input);
    }
    let key = prompt_for_key()?;
    if key.is_empty() {
        bail!("OpenRouter API key is required")
    }
    Ok(json!({"apiKey": key}))
}
fn prompt(message: &str) -> Result<String> {
    eprint!("{message}");
    io::stderr().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_owned())
}

fn is_interactive(a: &ShowArgs) -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal() && !a.json_output
}

fn login_requested(answer: &str) -> bool {
    matches!(answer.to_ascii_lowercase().as_str(), "" | "y" | "yes")
}

fn github_login_input(
    http: &dyn HttpClient,
    proc: &dyn ProcessRunner,
    ctx: &RequestContext,
) -> Result<Value> {
    use crate::providers::github_copilot::GitHubCopilotProvider as G;
    let accounts = G::discover_gh_accounts(proc, ctx);
    let mut selected = accounts.first().cloned();
    if accounts.len() > 1 {
        eprintln!("\nSelect a GitHub account:");
        for (i, user) in accounts.iter().enumerate() {
            eprintln!("{}) {user}", i + 1);
        }
        let choice = prompt("Enter choice [1]: ")?;
        let index: usize = if choice.is_empty() {
            1
        } else {
            choice.parse().unwrap_or(0)
        };
        selected = accounts.get(index.saturating_sub(1)).cloned();
        if selected.is_none() {
            bail!("A GitHub account selection is required")
        }
    }
    let mut source = "gh_cli";
    let token_user = (accounts.len() > 1)
        .then_some(selected.as_deref())
        .flatten();
    let token = if let Some(token) = G::gh_token(proc, token_user, ctx)? {
        eprintln!("✓ GitHub CLI token found");
        token
    } else {
        eprintln!("⚠ Could not load GitHub CLI token");
        source = "manual";
        let token = prompt("Enter GitHub token (or press Enter to skip): ")?;
        if token.is_empty() {
            bail!("GitHub token is required")
        }
        token
    };
    let include = prompt("Include work/organization Copilot credits? [y/N]: ")?;
    let mut organization = None;
    if matches!(include.to_lowercase().as_str(), "y" | "yes") {
        eprintln!("Discovering organizations...");
        let orgs = G::discover_organizations(http, &token, ctx).unwrap_or_default();
        if orgs.is_empty() {
            organization = Some(prompt(
                "Enter organization name (optional, for work quotas): ",
            )?)
            .filter(|s| !s.is_empty());
        } else {
            eprintln!(
                "Found {} organization(s)\n\nSelect an organization (or press Enter to skip):",
                orgs.len()
            );
            for (i, org) in orgs.iter().enumerate() {
                eprintln!("{}) {org}", i + 1);
            }
            eprintln!("0) Skip (personal only)");
            let choice = prompt("Enter choice [0]: ")?.parse::<usize>().unwrap_or(0);
            organization = choice.checked_sub(1).and_then(|i| orgs.get(i)).cloned();
        }
    }
    Ok(
        json!({"githubToken":token,"token_source":source,"github_account":selected,"organization":organization}),
    )
}
fn history(a: HistoryArgs) -> Result<()> {
    let c = Config::new(None);
    let h = HistoryManager::new(Some(c.history_db_path()))?;
    if a.summary {
        verbose_history_context(a.verbose, &h, &VerboseFilters::default());
        let info = h.get_database_info()?;
        if a.verbose {
            let records = h.get_history(None, None, None, None, None, None)?.len();
            eprintln!("[verbose] history result count: {records}");
        }
        return print_ok(crate::history::render_history_summary(&info));
    }
    let view = if a.heatmap {
        Some("heatmap")
    } else if a.chart {
        Some("chart")
    } else if a.calendar {
        Some("calendar")
    } else if a.bars {
        Some("bars")
    } else if a.stats {
        Some("stats")
    } else {
        None
    };
    if let Some(view) = view {
        let weekly = h.get_weekly_activity(a.account.as_deref(), a.provider.as_deref())?;
        if view == "stats" {
            let preset = a.preset.as_deref().or(Some("7d"));
            let filters = VerboseFilters {
                preset,
                since: a.since.as_deref(),
                until: a.until.as_deref(),
                account: a.account.as_deref(),
                provider: a.provider.as_deref(),
                quota: a.quota.as_deref(),
            };
            verbose_history_context(a.verbose, &h, &filters);
            let history = h.get_history(
                preset,
                a.since.as_deref(),
                a.until.as_deref(),
                a.account.as_deref(),
                a.provider.as_deref(),
                a.quota.as_deref(),
            )?;
            let aggregation = h.get_aggregation(
                preset,
                a.since.as_deref(),
                a.until.as_deref(),
                a.account.as_deref(),
                a.provider.as_deref(),
            )?;
            if a.verbose {
                eprintln!("[verbose] history record count: {}", history.len());
                eprintln!(
                    "[verbose] history aggregation result count: {}",
                    aggregation.len()
                );
                eprintln!(
                    "[verbose] history weekly result count: {}",
                    weekly.daily_per_account.len() + weekly.daily_totals.len()
                );
            }
            return print_ok(crate::history::render_stats(
                &history,
                &weekly,
                &aggregation,
            ));
        }
        let filters = VerboseFilters {
            preset: Some("7d"),
            since: None,
            until: None,
            account: a.account.as_deref(),
            provider: a.provider.as_deref(),
            quota: None,
        };
        verbose_history_context(a.verbose, &h, &filters);
        if a.verbose {
            eprintln!(
                "[verbose] history weekly result count: {}",
                weekly.daily_per_account.len() + weekly.daily_totals.len()
            );
        }
        return print_ok(crate::history::render_weekly(view, &weekly));
    }
    let preset = a
        .preset
        .as_deref()
        .or(if a.since.is_none() { Some("24h") } else { None });
    let filters = VerboseFilters {
        preset,
        since: a.since.as_deref(),
        until: a.until.as_deref(),
        account: a.account.as_deref(),
        provider: a.provider.as_deref(),
        quota: a.quota.as_deref(),
    };
    verbose_history_context(a.verbose, &h, &filters);
    let d = h.get_history(
        preset,
        a.since.as_deref(),
        a.until.as_deref(),
        a.account.as_deref(),
        a.provider.as_deref(),
        a.quota.as_deref(),
    )?;
    if a.verbose {
        eprintln!("[verbose] history result count: {}", d.len());
    }
    print_ok(if a.table {
        crate::history::render_history_table(&d)
    } else {
        crate::history::render_history_sparklines(&d)
    })
}
fn export(a: ExportArgs) -> Result<()> {
    let c = Config::new(None);
    let h = HistoryManager::new(Some(c.history_db_path()))?;
    let e = Exporter { history: &h };
    let f = ExportFilter {
        preset: a
            .preset
            .as_deref()
            .or(if a.since.is_none() { Some("7d") } else { None }),
        since: a.since.as_deref(),
        until: a.until.as_deref(),
        account_email: a.account.as_deref(),
        provider_type: a.provider.as_deref(),
        quota_name: a.quota.as_deref(),
    };
    verbose_export_context(a.verbose, &h, &f);
    let (s, info) = match a.format {
        Format::Csv => e.export_csv_with_info(a.output.as_deref(), &f)?,
        Format::Markdown => e.export_markdown_with_info(a.output.as_deref(), &f)?,
    };
    if a.verbose {
        eprintln!("[verbose] export record count: {}", info.record_count);
    }
    if let Some(p) = a.output {
        println!("Exported to {}", p.display())
    } else {
        print!("{s}")
    }
    Ok(())
}
#[derive(Default)]
struct VerboseFilters<'a> {
    preset: Option<&'a str>,
    since: Option<&'a str>,
    until: Option<&'a str>,
    account: Option<&'a str>,
    provider: Option<&'a str>,
    quota: Option<&'a str>,
}
fn verbose_history_context(verbose: bool, history: &HistoryManager, filters: &VerboseFilters<'_>) {
    if verbose {
        eprintln!("[verbose] history database: {}", resolved_db_path(history));
        eprintln!("[verbose] history filters: {}", verbose_filters(filters));
    }
}
fn verbose_export_context(verbose: bool, history: &HistoryManager, filter: &ExportFilter<'_>) {
    if verbose {
        eprintln!("[verbose] export database: {}", resolved_db_path(history));
        eprintln!(
            "[verbose] export filters: {}",
            verbose_filters(&VerboseFilters {
                preset: filter.preset,
                since: filter.since,
                until: filter.until,
                account: filter.account_email,
                provider: filter.provider_type,
                quota: filter.quota_name,
            })
        );
    }
}
fn resolved_db_path(history: &HistoryManager) -> String {
    history
        .storage
        .db_path
        .canonicalize()
        .unwrap_or_else(|_| history.storage.db_path.clone())
        .display()
        .to_string()
}
fn verbose_filters(filters: &VerboseFilters<'_>) -> String {
    [
        ("preset", filters.preset),
        ("since", filters.since),
        ("until", filters.until),
        ("account", filters.account),
        ("provider", filters.provider),
        ("quota", filters.quota),
    ]
    .into_iter()
    .map(|(name, value)| format!("{name}={}", redact_diagnostic_value(value)))
    .collect::<Vec<_>>()
    .join(", ")
}
fn redact_diagnostic_value(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "none".into();
    };
    let lower = value.to_ascii_lowercase();
    if lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.starts_with("sk-")
    {
        "<redacted>".into()
    } else {
        crate::providers::base::sanitize_diagnostic(value)
            .chars()
            .take(120)
            .collect()
    }
}
fn print_ok(s: String) -> Result<()> {
    print!("{s}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_key_input_is_tty_only_and_preserves_explicit_input() {
        let prompted = std::cell::Cell::new(false);
        let prompted_input = openrouter_login_input(Value::Null, true, || {
            prompted.set(true);
            Ok("sk-or-test".into())
        })
        .unwrap();
        assert!(prompted.get());
        assert_eq!(prompted_input["apiKey"], "sk-or-test");

        let explicit = json!({"apiKey": "from-json"});
        assert_eq!(
            openrouter_login_input(explicit.clone(), true, || -> Result<String> {
                panic!("explicit input must not prompt")
            })
            .unwrap(),
            explicit
        );
        assert!(
            openrouter_login_input(Value::Null, false, || -> Result<String> {
                panic!("non-TTY login must not prompt")
            })
            .unwrap()
            .is_null()
        );
    }

    #[test]
    fn missing_accounts_login_prompt_defaults_to_yes() {
        for answer in ["", "y", "Y", "yes", "YES"] {
            assert!(login_requested(answer), "{answer:?} should request login");
        }
        for answer in ["n", "no", "anything else"] {
            assert!(!login_requested(answer), "{answer:?} should decline login");
        }
    }
}
