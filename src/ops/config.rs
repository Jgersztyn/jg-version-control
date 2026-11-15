//! `jg config`, `jg alias` and `jg shell-init` (shell-level shortcuts like `gs`).

use super::{Message, Report, Style};
use crate::core::Repo;
use crate::core::config::{Config, ConfigFile, global_path};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Built-in `jg` aliases, used unless overridden by `alias.<name>` config.
pub const DEFAULT_ALIASES: &[(&str, &str)] = &[
    ("st", "status"),
    ("co", "checkout"),
    ("sw", "checkout"),
    ("br", "branch"),
    ("ci", "commit"),
    ("amend", "commit --amend --no-edit"),
    ("unstage", "restore --staged"),
    ("last", "log -n 1"),
    ("lg", "log --oneline --all"),
    ("undo", "reset --soft HEAD~1"),
];

/// Default shell shortcuts printed by `jg shell-init` (override with `shell-alias.<name>`).
pub const DEFAULT_SHELL_ALIASES: &[(&str, &str)] = &[
    ("gs", "status"),
    ("ga", "add"),
    ("gaa", "add -A"),
    ("gc", "commit"),
    ("gcm", "commit -m"),
    ("gca", "commit --amend --no-edit"),
    ("gco", "checkout"),
    ("gcb", "checkout -b"),
    ("gb", "branch"),
    ("gl", "log --oneline -n 20"),
    ("gd", "diff"),
    ("gds", "diff --staged"),
    ("gst", "stash"),
    ("gsp", "stash pop"),
    ("gp", "push"),
    ("gpl", "pull"),
    ("gm", "merge"),
];

fn load_repo_opt() -> Option<Repo> {
    std::env::current_dir().ok().and_then(|d| Repo::discover(&d).ok())
}

// ---------------------------------------------------------------- config

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct ConfigOpts {
    /// Key such as user.name, user.email, ai.command, remote.origin.url
    pub key: Option<String>,
    /// Value to set (omit to read the key)
    pub value: Option<String>,
    /// Use the per-user config file instead of the repository's
    #[arg(long)]
    pub global: bool,
    /// List all settings
    #[arg(short = 'l', long)]
    pub list: bool,
    /// Remove the key
    #[arg(long)]
    pub unset: bool,
}

#[derive(Serialize)]
pub struct ConfigList {
    pub entries: Vec<serde_json::Value>,
}

impl Report for ConfigList {
    fn human(&self, _: Style) -> String {
        self.entries
            .iter()
            .map(|e| format!("{}={}", e["key"].as_str().unwrap_or(""), e["value"].as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub enum ConfigResult {
    List(ConfigList),
    Done(Message),
}

pub fn config(opts: ConfigOpts) -> Result<ConfigResult> {
    let repo = load_repo_opt();
    let file_path = if opts.global {
        global_path()
    } else {
        match &repo {
            Some(r) => r.dir.join("config"),
            None if opts.key.is_some() && opts.value.is_none() && !opts.unset => global_path(),
            None if opts.list => global_path(),
            None => bail!("not in a jg repository; use --global to change your user settings"),
        }
    };
    if opts.list || opts.key.is_none() {
        let entries = if opts.global {
            ConfigFile::load(&file_path)
                .entries()
                .into_iter()
                .map(|(k, v)| json!({ "key": k, "value": v, "scope": "global" }))
                .collect()
        } else {
            Config::load(repo.as_ref().map(|r| r.dir.join("config")).as_deref())
                .entries()
                .into_iter()
                .map(|(k, v, s)| json!({ "key": k, "value": v, "scope": s }))
                .collect()
        };
        return Ok(ConfigResult::List(ConfigList { entries }));
    }
    let key = opts.key.clone().unwrap();
    let mut file = ConfigFile::load(&file_path);
    if opts.unset {
        if !file.unset(&key)? {
            bail!("key '{key}' is not set");
        }
        file.save()?;
        return Ok(ConfigResult::Done(Message::new(format!("unset {key}"), json!({ "unset": key }))));
    }
    match &opts.value {
        Some(v) => {
            file.set(&key, v)?;
            file.save()?;
            Ok(ConfigResult::Done(Message::new(String::new(), json!({ "key": key, "value": v, "file": file_path }))))
        }
        None => {
            let layered = Config::load(repo.as_ref().map(|r| r.dir.join("config")).as_deref());
            let v = if opts.global { file.get(&key) } else { layered.get(&key) };
            match v {
                Some(v) => Ok(ConfigResult::Done(Message::new(v.clone(), json!({ "key": key, "value": v })))),
                None => bail!("key '{key}' is not set"),
            }
        }
    }
}

// ---------------------------------------------------------------- aliases

#[derive(clap::Subcommand, Deserialize, Debug)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum AliasCmd {
    /// List jg aliases and shell shortcuts (default)
    List,
    /// Define an alias. `jg alias set st status` makes `jg st` work.
    /// With --shell, defines a shell shortcut: `jg alias set --shell gs status` makes `gs` work.
    Set {
        name: String,
        /// The jg command line the alias expands to
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true, trailing_var_arg = true)]
        expansion: Vec<String>,
        /// Define a shell shortcut (used by `jg shell-init`) instead of a jg alias
        #[arg(long)]
        shell: bool,
    },
    /// Remove an alias (or, with --shell, a shell shortcut)
    Remove {
        name: String,
        #[arg(long)]
        shell: bool,
    },
}

#[derive(Serialize)]
pub struct AliasList {
    pub aliases: Vec<(String, String)>,
    pub shell_aliases: Vec<(String, String)>,
}

impl Report for AliasList {
    fn human(&self, s: Style) -> String {
        let mut o = format!("{}\n", s.bold("jg aliases (jg <alias>):"));
        for (k, v) in &self.aliases {
            o.push_str(&format!("  {k:<10} = jg {v}\n"));
        }
        o.push_str(&format!("\n{}\n", s.bold("shell shortcuts (enable with: eval \"$(jg shell-init)\"):")));
        for (k, v) in &self.shell_aliases {
            o.push_str(&format!("  {k:<10} = jg {v}\n"));
        }
        o.trim_end().to_string()
    }
}

/// Effective jg aliases: defaults overlaid with config.
pub fn aliases(cfg: &Config) -> Vec<(String, String)> {
    merged(cfg, "alias", DEFAULT_ALIASES)
}

pub fn shell_aliases(cfg: &Config) -> Vec<(String, String)> {
    merged(cfg, "shell-alias", DEFAULT_SHELL_ALIASES)
}

fn merged(cfg: &Config, section: &str, defaults: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = defaults.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let prefix = format!("{section}.");
    for (k, v, _) in cfg.entries() {
        if let Some(name) = k.strip_prefix(&prefix) {
            out.retain(|(n, _)| n != name);
            // An empty value disables a default.
            if !v.trim().is_empty() {
                out.push((name.to_string(), v));
            }
        }
    }
    out.sort();
    out
}

pub enum AliasResult {
    List(AliasList),
    Done(Message),
}

/// Aliases are stored in the per-user (global) config so they work in every repo.
pub fn alias(cmd: Option<AliasCmd>) -> Result<AliasResult> {
    let mut file = ConfigFile::load(&global_path());
    match cmd.unwrap_or(AliasCmd::List) {
        AliasCmd::List => {
            let cfg = Config::load(load_repo_opt().map(|r| r.dir.join("config")).as_deref());
            Ok(AliasResult::List(AliasList { aliases: aliases(&cfg), shell_aliases: shell_aliases(&cfg) }))
        }
        AliasCmd::Set { name, expansion, shell } => {
            if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '.') {
                bail!("invalid alias name '{name}'");
            }
            let value = expansion.join(" ");
            let value = value.strip_prefix("jg ").unwrap_or(&value).to_string();
            let section = if shell { "shell-alias" } else { "alias" };
            file.set(&format!("{section}.{name}"), &value)?;
            file.save()?;
            let hint = if shell { "\nReload your shell (or run: eval \"$(jg shell-init)\") to use it." } else { "" };
            Ok(AliasResult::Done(Message::new(
                format!("{} '{name}' = jg {value}{hint}", if shell { "shell shortcut" } else { "alias" }),
                json!({ "name": name, "expansion": value, "shell": shell }),
            )))
        }
        AliasCmd::Remove { name, shell } => {
            let section = if shell { "shell-alias" } else { "alias" };
            let defaults = if shell { DEFAULT_SHELL_ALIASES } else { DEFAULT_ALIASES };
            let removed = file.unset(&format!("{section}.{name}"))?;
            if defaults.iter().any(|(k, _)| *k == name) {
                // Disable the built-in default with an empty value.
                file.set(&format!("{section}.{name}"), "")?;
            } else if !removed {
                bail!("no such alias '{name}'");
            }
            file.save()?;
            Ok(AliasResult::Done(Message::new(format!("removed '{name}'"), json!({ "removed": name, "shell": shell }))))
        }
    }
}

/// Expand a jg alias in argv (`jg st -s` -> `jg status -s`). Built-in commands win.
pub fn expand_alias(args: Vec<String>, builtin: &[String]) -> Result<Vec<String>> {
    // Find the subcommand position (skip global flags like --json / --color X / -C dir).
    let mut i = 1;
    while i < args.len() && args[i].starts_with('-') {
        if ["--color", "-C"].contains(&args[i].as_str()) {
            i += 1;
        }
        i += 1;
    }
    if i >= args.len() || builtin.contains(&args[i]) {
        return Ok(args);
    }
    let cfg = Config::load(load_repo_opt().map(|r| r.dir.join("config")).as_deref());
    let Some((_, exp)) = aliases(&cfg).into_iter().find(|(n, _)| *n == args[i]) else {
        return Ok(args);
    };
    let words = split_words(&exp).context("bad alias")?;
    if words.first().is_some_and(|w| *w == args[i]) {
        bail!("alias '{}' expands to itself", args[i]);
    }
    let mut out = args[..i].to_vec();
    out.extend(words);
    out.extend_from_slice(&args[i + 1..]);
    Ok(out)
}

/// Minimal shell-like word splitting with single/double quotes.
pub fn split_words(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    Some(out)
}

// ---------------------------------------------------------------- shell-init

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct ShellInitOpts {
    /// bash, zsh, fish or powershell (default: detect from $SHELL)
    pub shell: Option<String>,
}

/// Print shell code defining the shortcuts. Add to your shell profile:
/// bash/zsh: `eval "$(jg shell-init)"`; PowerShell: `jg shell-init powershell | Out-String | Invoke-Expression`.
pub fn shell_init(opts: ShellInitOpts) -> Result<String> {
    let cfg = Config::load(load_repo_opt().map(|r| r.dir.join("config")).as_deref());
    let shell = opts.shell.unwrap_or_else(|| {
        if cfg!(windows) {
            "powershell".into()
        } else {
            std::env::var("SHELL")
                .ok()
                .and_then(|s| s.rsplit('/').next().map(String::from))
                .unwrap_or_else(|| "bash".into())
        }
    });
    let mut o = String::new();
    for (name, exp) in shell_aliases(&cfg) {
        match shell.as_str() {
            "bash" | "zsh" | "sh" => {
                // Functions (not aliases) pass arguments through. `unalias` runs on its own
                // line first, because bash expands aliases when it reads a line.
                o.push_str(&format!("unalias {name} 2>/dev/null\nfunction {name} {{ jg {exp} \"$@\"; }}\n"));
            }
            "fish" => o.push_str(&format!("function {name}; jg {exp} $argv; end\n")),
            "powershell" | "pwsh" => {
                o.push_str(&format!("Remove-Item Alias:{name} -Force -ErrorAction SilentlyContinue\nfunction global:{name} {{ jg {exp} @args }}\n"))
            }
            other => bail!("unsupported shell '{other}' (use bash, zsh, fish or powershell)"),
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(split_words("commit -m 'a b' \"c\"").unwrap(), vec!["commit", "-m", "a b", "c"]);
    }
}
