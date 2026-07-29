<div align="center">
  <img src="limitwatch.svg" alt="LimitWatch logo" width="128" height="128">
  <h1>LimitWatch</h1>
  <p><strong>Keep every AI coding quota in sight.</strong></p>
  <p>A fast Rust CLI for GitHub Copilot, OpenAI Codex, and OpenRouter.</p>
</div>

<br>

## Features

- Interactive login and multi-account monitoring.
- Account aliases and groups, plus account, provider, and quota filtering.
- Standard, compact, and JSON quota output with reset countdowns.
- SQLite-backed quota history, charts, summaries, and CSV or Markdown export.
- Dynamic shell completions for bash, zsh, and fish.
- Durable account, configuration, and history data in
  `~/.config/limitwatch/`.

## Supported providers

| Provider | What is monitored | Authentication |
| --- | --- | --- |
| **GitHub Copilot** | Personal or organization Copilot usage | `gh auth token` discovery or GitHub token |
| **OpenAI Codex** | Codex plan usage windows | Local credentials or OpenAI device login |
| **OpenRouter** | Remaining credits and key usage | API key |

Google and Chutes.ai accounts are not supported. Existing records for those
providers are left intact in account storage but are ignored by selection,
fetching, history filters, and completions.

## Install

On Linux x86_64, install the latest release binary with:

```sh
curl -fsSL https://raw.githubusercontent.com/lucashutch/limitwatch/main/scripts/install.sh | sh
```

The installer verifies the release checksum and installs `limitwatch` to
`~/.local/bin`. Set `LIMITWATCH_INSTALL_DIR` to use another directory.

Alternatively, [install Rust](https://www.rust-lang.org/tools/install) 1.88 or
newer and build LimitWatch from GitHub:

```sh
cargo install --git https://github.com/lucashutch/limitwatch.git --locked
```

The executable is installed as `limitwatch`. To install from a local checkout
instead, run `cargo install --path . --locked` in the repository root.

## Usage

```sh
# Add an account interactively
limitwatch --login

# Show all supported accounts
limitwatch

# Filter or change output format
limitwatch --provider openrouter --compact
limitwatch --account work --json --timings

# Review or export history
limitwatch history --preset 7d --table
limitwatch export --format markdown --output quotas.md
```

Run `limitwatch --help` for the complete command reference.

### Main options

```text
-a, --account <ID>         Repeatable email or alias filter
    --alias <ALIAS>        Set or clear an account alias
-g, --group <GROUP>        Filter by group or update account metadata
-p, --provider <TYPE>      Repeatable provider filter
-q, --query <TEXT>         Repeatable quota text filter
-r, --refresh              Refresh provider data
-s, --show-all             Include normally hidden quotas
-c, --compact              Use compact output
-j, --json                 Use machine-readable output
-l, --login                Add or update an account
    --logout               Remove the selected account
    --logout-all           Remove all accounts
    --no-record            Do not write this fetch to history
    --verbose              Enable diagnostic output
    --timings              Include timings in JSON output
    --max-age-ms <MS>      Set the overall fetch deadline (default: 4000)
    --cache-ttl <SEC>      Override the cached-quota lifetime
```

Additional commands include:

- `history` for tables, summaries, charts, calendars, bars, and statistics.
- `export` for CSV or Markdown history exports.
- `completion bash|zsh|fish` for shell completion scripts.

## Authentication

Running `limitwatch --login` prompts for a provider. You can select one
directly with `--provider`.

- **GitHub Copilot:** Uses `gh auth token` when the GitHub CLI is installed and
  authenticated, or accepts a GitHub token. Selecting an organization during
  login monitors that organization's credits instead of personal credits.
- **OpenAI Codex:** Discovers local OpenCode and Codex credentials first, then
  offers OpenAI device authorization when needed.
- **OpenRouter:** Prompts for and validates an API key in interactive sessions.

Non-interactive login can provide explicit input through
`LIMITWATCH_LOGIN_JSON`, for example:

```sh
LIMITWATCH_LOGIN_JSON='{"apiKey":"..."}' \
  limitwatch --login --provider openrouter
```

## Data and security

LimitWatch stores `accounts.json`, `config.json`, and `history.db` in
`~/.config/limitwatch/` by default. Unknown JSON fields are retained during
normal updates, and file writes use atomic replacement.

Account storage contains tokens and API keys in plaintext with the permissions
inherited from the configuration directory. Do not commit, print, or share
these files. Restrict the directory's permissions and redact diagnostics before
sharing them. JSON quota output does not intentionally include credentials but
may contain messages returned by providers.

## Shell completions

Generate a completion script using your shell's normal installation path:

```sh
limitwatch completion bash > ~/.local/share/bash-completion/completions/limitwatch
limitwatch completion zsh > ~/.zfunc/_limitwatch
limitwatch completion fish > ~/.config/fish/completions/limitwatch.fish
```

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo run -- --help
```

Integration tests use sanitized fixtures and mocked responses; they do not
require credentials or live provider APIs. SQLite is bundled, so no system
SQLite installation is required.

### Versioning

Release builds derive `limitwatch --version` from Git tags with
[`vergen-git2`](https://crates.io/crates/vergen-git2). An exact `v0.7.0` tag
reports `limitwatch 0.7.0`, while later development builds include the commit
distance, abbreviated revision, and dirty state. Builds without Git metadata
fall back to the package version in `Cargo.toml`.

## License

[MIT](LICENSE)

## Upgrading from older Python releases

The Python implementation has been retired. Existing users must remove the old
tool and reinstall LimitWatch so their shell resolves the Rust executable:

```sh
uv tool uninstall limitwatch
cargo install --git https://github.com/lucashutch/limitwatch.git --locked
```

Your data in `~/.config/limitwatch/` remains in place. Run
`limitwatch --version` after reinstalling to confirm the new executable is
active.
