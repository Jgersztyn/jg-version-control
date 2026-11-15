//! `jg remote` — manage named remotes (stored as `remote.<name>.url` in `.jg/config`).

use super::{Message, Report, Style, open_repo};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(clap::Subcommand, Deserialize, Debug)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum RemoteCmd {
    /// Add a remote: a local path, `user@host:path`, or `ssh://user@host[:port]/path`
    Add { name: String, url: String },
    /// Remove a remote and its remote-tracking branches
    Remove { name: String },
    /// Change a remote's URL
    SetUrl { name: String, url: String },
    /// List remotes (default)
    List,
}

#[derive(Serialize)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct RemoteList {
    pub remotes: Vec<RemoteInfo>,
}

impl Report for RemoteList {
    fn human(&self, _: Style) -> String {
        self.remotes.iter().map(|r| format!("{}\t{}", r.name, r.url)).collect::<Vec<_>>().join("\n")
    }
}

pub enum RemoteResult {
    List(RemoteList),
    Done(Message),
}

pub fn run(cmd: Option<RemoteCmd>) -> Result<RemoteResult> {
    let repo = open_repo()?;
    let mut cfg = repo.repo_config_file();
    match cmd.unwrap_or(RemoteCmd::List) {
        RemoteCmd::List => {
            let remotes = cfg
                .subsections("remote")
                .into_iter()
                .map(|n| RemoteInfo { url: cfg.get(&format!("remote.{n}.url")).unwrap_or_default(), name: n })
                .collect();
            Ok(RemoteResult::List(RemoteList { remotes }))
        }
        RemoteCmd::Add { name, url } => {
            crate::core::refs::check_name(&name)?;
            if cfg.get(&format!("remote.{name}.url")).is_some() {
                bail!("remote '{name}' already exists");
            }
            cfg.set(&format!("remote.{name}.url"), &url)?;
            cfg.save()?;
            Ok(RemoteResult::Done(Message::new(
                format!("Added remote '{name}' -> {url}"),
                json!({ "added": name, "url": url }),
            )))
        }
        RemoteCmd::SetUrl { name, url } => {
            if cfg.get(&format!("remote.{name}.url")).is_none() {
                bail!("no such remote '{name}'");
            }
            cfg.set(&format!("remote.{name}.url"), &url)?;
            cfg.save()?;
            Ok(RemoteResult::Done(Message::new(
                format!("Remote '{name}' now points to {url}"),
                json!({ "remote": name, "url": url }),
            )))
        }
        RemoteCmd::Remove { name } => {
            if !cfg.remove_section("remote", Some(&name)) {
                bail!("no such remote '{name}'");
            }
            cfg.save()?;
            for (r, _) in repo.refs.list(&format!("refs/remotes/{name}/"))? {
                repo.refs.delete(&r)?;
            }
            Ok(RemoteResult::Done(Message::new(format!("Removed remote '{name}'"), json!({ "removed": name }))))
        }
    }
}
