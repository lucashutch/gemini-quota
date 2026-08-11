use crate::{auth::AuthManager, config::Config};
use clap::Command;
use clap_complete::{
    generate,
    shells::{Bash, Fish, Zsh},
};
use std::{collections::BTreeSet, env, io::Cursor, path::PathBuf};
pub fn generate_script(shell: &str, mut cmd: Command) -> anyhow::Result<()> {
    let mut generated = Vec::new();
    match shell {
        "bash" => generate(
            Bash,
            &mut cmd,
            "limitwatch",
            &mut Cursor::new(&mut generated),
        ),
        "zsh" => generate(
            Zsh,
            &mut cmd,
            "limitwatch",
            &mut Cursor::new(&mut generated),
        ),
        "fish" => generate(
            Fish,
            &mut cmd,
            "limitwatch",
            &mut Cursor::new(&mut generated),
        ),
        _ => anyhow::bail!("unsupported shell: {shell}"),
    }
    let dynamic = match shell {
        "bash" => {
            r#"
_limitwatch_dynamic() {
    _limitwatch "$@"
    local cur="${COMP_WORDS[COMP_CWORD]}" prev="${COMP_WORDS[COMP_CWORD-1]}" kind=""
    case "$prev" in
        -a|--account|--select-account) kind=account ;;
        -g|--group) kind=group ;;
        -q|--query|--quota) kind=quota ;;
        -p|--provider) kind=provider ;;
        --preset) kind=preset ;;
        --format) kind=format ;;
    esac
    if [[ -n "$kind" ]]; then
        local values
        values="$(command limitwatch complete "$kind" "$cur" 2>/dev/null)"
        COMPREPLY=( $(compgen -W "$values" -- "$cur") )
    fi
}
complete -F _limitwatch_dynamic -o nosort -o bashdefault -o default limitwatch
"#
        }
        "zsh" => {
            r#"
_limitwatch_dynamic() {
    _limitwatch "$@"
    local kind=""
    case "${words[CURRENT-1]}" in
        -a|--account|--select-account) kind=account ;;
        -g|--group) kind=group ;;
        -q|--query|--quota) kind=quota ;;
        -p|--provider) kind=provider ;;
        --preset) kind=preset ;;
        --format) kind=format ;;
    esac
    if [[ -n "$kind" ]]; then
        compadd -- ${(f)"$(command limitwatch complete \"$kind\" \"${words[CURRENT]}\" 2>/dev/null)"}
    fi
}
compdef _limitwatch_dynamic limitwatch
"#
        }
        "fish" => {
            r#"
function __limitwatch_dynamic
    command limitwatch complete $argv[1] (commandline -ct) 2>/dev/null
end
complete -c limitwatch -l account -r -a '(__limitwatch_dynamic account)'
complete -c limitwatch -l select-account -r -a '(__limitwatch_dynamic account)'
complete -c limitwatch -l group -r -a '(__limitwatch_dynamic group)'
complete -c limitwatch -l query -r -a '(__limitwatch_dynamic quota)'
complete -c limitwatch -l quota -r -a '(__limitwatch_dynamic quota)'
complete -c limitwatch -l provider -r -a '(__limitwatch_dynamic provider)'
complete -c limitwatch -l preset -r -a '(__limitwatch_dynamic preset)'
complete -c limitwatch -l format -r -a '(__limitwatch_dynamic format)'
"#
        }
        _ => unreachable!(),
    };
    std::io::Write::write_all(&mut std::io::stdout(), &generated)?;
    std::io::Write::write_all(&mut std::io::stdout(), dynamic.as_bytes())?;
    Ok(())
}
pub fn candidates(kind: &str, prefix: &str) -> Vec<String> {
    let config_dir = env::var_os("LIMITWATCH_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_CONFIG_HOME").map(|path| PathBuf::from(path).join("limitwatch"))
        })
        .unwrap_or_else(|| Config::new(None).config_dir);
    let auth = AuthManager::new(config_dir.join("accounts.json"));
    let mut out = BTreeSet::new();
    match kind {
        "account" => {
            for (_, a) in auth.supported_accounts() {
                out.insert(a.email.clone());
                if let Some(x) = &a.alias {
                    out.insert(x.clone());
                }
            }
        }
        "group" => {
            for (_, a) in auth.supported_accounts() {
                if let Some(x) = &a.group {
                    out.insert(x.clone());
                }
            }
        }
        "provider" => {
            for x in ["github_copilot", "openai", "opencode", "openrouter"] {
                out.insert(x.into());
            }
        }
        "quota" => {
            for (_, a) in auth.supported_accounts() {
                if let Some(q) = a.extra.get("cachedQuotas").and_then(|x| x.as_array()) {
                    for q in q {
                        for k in ["name", "display_name"] {
                            if let Some(x) = q.get(k).and_then(|x| x.as_str()) {
                                out.insert(x.into());
                            }
                        }
                    }
                }
            }
        }
        "format" => {
            for x in ["csv", "markdown"] {
                out.insert(x.into());
            }
        }
        "preset" => {
            for x in ["24h", "7d", "30d", "90d"] {
                out.insert(x.into());
            }
        }
        "view" => {
            for x in ["heatmap", "chart", "calendar", "bars", "stats"] {
                out.insert(x.into());
            }
        }
        _ => {}
    }
    out.into_iter().filter(|x| x.starts_with(prefix)).collect()
}
