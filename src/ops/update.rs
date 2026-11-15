//! `jg self-update` — update the jg binary from your own release location.
//!
//! The release location (`update.source`) is a directory or an http(s) URL holding
//! `latest.json` (written by `scripts/release.sh`):
//!
//! ```json
//! { "version": "0.2.0",
//!   "assets": { "linux-x86_64":   { "file": "jg-0.2.0-linux-x86_64",       "sha256": "..." },
//!               "windows-x86_64": { "file": "jg-0.2.0-windows-x86_64.exe", "sha256": "..." } } }
//! ```
//! Downloads use the system `curl` (present on Ubuntu and Windows 10+), so jg needs
//! no TLS library. Installs from the .deb/APT repo should update with apt instead.

use super::Message;
use crate::core::config::Config;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct UpdateOpts {
    /// Only check whether a newer version exists
    #[arg(long)]
    pub check: bool,
    /// Release directory or URL (default: `update.source` config)
    #[arg(long)]
    pub source: Option<String>,
    /// Reinstall even if already up to date, or if installed by a package manager
    #[arg(long)]
    pub force: bool,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: String,
    assets: std::collections::BTreeMap<String, Asset>,
}

#[derive(Deserialize)]
struct Asset {
    file: String,
    sha256: String,
}

pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn fetch(source: &str, name: &str) -> Result<Vec<u8>> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let url = format!("{}/{name}", source.trim_end_matches('/'));
        let out = std::process::Command::new("curl")
            .args(["-fsSL", "--retry", "2", &url])
            .output()
            .context("running curl (needed for downloads)")?;
        if !out.status.success() {
            bail!("download failed: {url}\n{}", String::from_utf8_lossy(&out.stderr));
        }
        Ok(out.stdout)
    } else {
        let p = Path::new(source).join(name);
        std::fs::read(&p).with_context(|| format!("reading {}", p.display()))
    }
}

/// Compare dotted numeric versions ("0.10.1" > "0.9.3").
pub fn newer(a: &str, b: &str) -> bool {
    let parse =
        |s: &str| -> Vec<u64> { s.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect() };
    parse(a) > parse(b)
}

pub fn run(opts: UpdateOpts) -> Result<Message> {
    let cfg = Config::load(None);
    let source = opts
        .source
        .clone()
        .or_else(|| std::env::var("JG_UPDATE_SOURCE").ok())
        .or_else(|| cfg.get("update.source"))
        .context("no update source configured. Point jg at your release folder or URL, e.g.:\n  jg config --global update.source https://myserver.example/jg\n  jg config --global update.source /mnt/share/jg-releases")?;
    let manifest: Manifest =
        serde_json::from_slice(&fetch(&source, "latest.json")?).context("latest.json is invalid")?;
    let available = newer(&manifest.version, VERSION);
    if opts.check || (!available && !opts.force) {
        let text = if available {
            format!(
                "jg {} is available (you have {VERSION}). Run 'jg self-update'.{}",
                manifest.version,
                if manifest.notes.is_empty() { String::new() } else { format!("\n\n{}", manifest.notes) }
            )
        } else {
            format!("jg {VERSION} is up to date.")
        };
        return Ok(Message::new(
            text,
            json!({ "current": VERSION, "latest": manifest.version, "update_available": available }),
        ));
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    if !opts.force && cfg!(unix) && exe.starts_with("/usr/") {
        bail!(
            "jg was installed by a package manager ({}).\nUpdate with: sudo apt update && sudo apt install --only-upgrade jg\n(or pass --force to overwrite anyway)",
            exe.display()
        );
    }
    let key = platform_key();
    let asset =
        manifest.assets.get(&key).with_context(|| format!("release {} has no build for {key}", manifest.version))?;
    let data = fetch(&source, &asset.file)?;
    let digest = crate::util::sha256_hex(&data);
    if !digest.eq_ignore_ascii_case(&asset.sha256) {
        bail!("checksum mismatch for {} (expected {}, got {digest}); not installing", asset.file, asset.sha256);
    }
    replace_exe(&exe, &data)?;
    Ok(Message::new(
        format!("Updated jg {VERSION} -> {}", manifest.version),
        json!({ "previous": VERSION, "installed": manifest.version, "path": exe }),
    ))
}

fn replace_exe(exe: &Path, data: &[u8]) -> Result<()> {
    let dir = exe.parent().context("bad executable path")?;
    let tmp = dir.join(".jg-update.tmp");
    std::fs::write(&tmp, data).with_context(|| format!("cannot write to {} (permissions?)", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, exe)?;
    }
    #[cfg(windows)]
    {
        // A running .exe can't be overwritten on Windows, but it can be renamed.
        let old = exe.with_extension("exe.old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old)?;
        std::fs::rename(&tmp, exe)?;
    }
    Ok(())
}

/// Remove a leftover `jg.exe.old` from a previous Windows self-update.
pub fn cleanup_old() {
    if cfg!(windows)
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_compare() {
        assert!(super::newer("0.10.0", "0.9.9"));
        assert!(!super::newer("0.1.0", "0.1.0"));
        assert!(super::newer("1.0.0", "0.99.0"));
    }
}
