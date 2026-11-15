//! Moving objects and refs between repositories.
//!
//! Two transports share one interface ([`Remote`]):
//! - **Local**: the other repo is a directory on this machine (or a mounted share).
//! - **SSH**: runs `jg serve <path>` on the server via the system `ssh` binary and
//!   talks a small framed protocol over stdin/stdout. Requires `jg` on the server.
//!
//! Wire protocol (SSH): every frame is a 4-byte big-endian length + payload.
//! Requests/responses are JSON frames; objects travel as binary frames of
//! `20-byte oid || zlib-compressed loose object`, terminated by an empty frame.

use crate::core::history::{is_ancestor, objects_to_send};
use crate::core::odb::Odb;
use crate::core::{Oid, Repo};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// What a remote repository advertises.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Advert {
    /// Full ref name -> id, for `refs/heads/*` and `refs/tags/*`.
    pub refs: BTreeMap<String, String>,
    /// The branch the remote's HEAD points at (e.g. `refs/heads/main`).
    pub head: Option<String>,
}

impl Advert {
    pub fn get(&self, name: &str) -> Option<Oid> {
        self.refs.get(name).and_then(|s| Oid::from_hex(s).ok())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefUpdate {
    pub name: String,
    /// What the client believes the ref currently is (ZERO = should not exist).
    pub old: String,
    /// New value (ZERO = delete).
    pub new: String,
    pub force: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefResult {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

pub trait Remote {
    fn advertise(&mut self) -> Result<Advert>;
    /// Download objects reachable from `wants` and not from `haves` into `odb`.
    fn fetch(&mut self, wants: &[Oid], haves: &[Oid], odb: &Odb) -> Result<usize>;
    /// Upload objects then apply ref updates atomically per ref.
    fn push(&mut self, objects: &[Oid], local: &Odb, updates: &[RefUpdate]) -> Result<Vec<RefResult>>;
}

// ---------------------------------------------------------------- URLs

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Url {
    Local(PathBuf),
    Ssh { host: String, port: Option<u16>, path: String },
}

pub fn parse_url(url: &str) -> Url {
    if let Some(rest) = url.strip_prefix("ssh://") {
        let (hostport, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_string()),
            None => (rest, ".".to_string()),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), p.parse().ok()),
            _ => (hostport.to_string(), None),
        };
        // ssh://host/~/repo -> ~/repo (relative to home)
        let path = path.strip_prefix("/~").map(|p| format!("~{p}")).unwrap_or(path);
        return Url::Ssh { host, port, path };
    }
    if let Some(p) = url.strip_prefix("file://") {
        return Url::Local(PathBuf::from(p));
    }
    // scp-like `user@host:path` (but not a Windows drive like `C:\x`).
    if let Some(colon) = url.find(':') {
        let before = &url[..colon];
        let is_drive = before.len() == 1 && before.chars().all(|c| c.is_ascii_alphabetic());
        if !is_drive
            && !before.contains('/')
            && !before.contains('\\')
            && !before.is_empty()
            && !std::path::Path::new(url).exists()
        {
            return Url::Ssh { host: before.to_string(), port: None, path: url[colon + 1..].to_string() };
        }
    }
    Url::Local(PathBuf::from(url))
}

pub fn connect(repo: &Repo, remote_name: Option<&str>, url: &str) -> Result<Box<dyn Remote>> {
    match parse_url(url) {
        Url::Local(p) => Ok(Box::new(LocalRemote {
            repo: Repo::open(&p).with_context(|| format!("opening remote repository '{url}'"))?,
        })),
        Url::Ssh { host, port, path } => {
            let jg_path = remote_name
                .and_then(|n| repo.config.get(&format!("remote.{n}.jgpath")))
                .unwrap_or_else(|| "jg".to_string());
            let ssh = std::env::var("JG_SSH")
                .ok()
                .or_else(|| repo.config.get("core.sshCommand"))
                .unwrap_or_else(|| "ssh".into());
            Ok(Box::new(SshRemote::spawn(&ssh, &host, port, &format!("{jg_path} serve {}", shell_quote(&path)))?))
        }
    }
}

fn shell_quote(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        // Leave ~ unquoted so the remote shell expands it.
        return format!("~/{}", shell_quote(rest));
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------- server side

/// Refs a repository advertises to clients.
pub fn advertise(repo: &Repo) -> Result<Advert> {
    let mut refs = BTreeMap::new();
    for prefix in ["refs/heads/", "refs/tags/"] {
        for (name, oid) in repo.refs.list(prefix)? {
            refs.insert(name, oid.to_hex());
        }
    }
    let head = match repo.head()? {
        crate::core::refs::Head::Branch(r) => Some(r),
        _ => None,
    };
    Ok(Advert { refs, head })
}

/// Apply ref updates after objects have been received.
pub fn apply_updates(repo: &Repo, updates: &[RefUpdate]) -> Result<Vec<RefResult>> {
    let _lock = repo.lock()?;
    let checked_out = if repo.is_bare() { None } else { crate::ops::branch::head_ref(repo)? };
    let who = repo.who();
    let mut results = Vec::new();
    for u in updates {
        let new = Oid::from_hex(&u.new)?;
        let old = Oid::from_hex(&u.old)?;
        let current = repo.refs.read(&u.name)?.unwrap_or(Oid::ZERO);
        let fail = |m: &str| RefResult { name: u.name.clone(), ok: false, message: m.to_string() };
        if !(u.name.starts_with("refs/heads/") || u.name.starts_with("refs/tags/")) {
            results.push(fail("only refs/heads/* and refs/tags/* can be pushed"));
            continue;
        }
        if checked_out.as_deref() == Some(u.name.as_str()) {
            results.push(fail("branch is checked out in the remote work tree (push to a bare repo: jg init --bare)"));
            continue;
        }
        if current != old && !u.force {
            results.push(fail("remote ref changed since last fetch (fetch first)"));
            continue;
        }
        if new.is_zero() {
            repo.refs.delete(&u.name)?;
            results.push(RefResult { name: u.name.clone(), ok: true, message: "deleted".into() });
            continue;
        }
        if !repo.odb.exists(&new) {
            results.push(fail("missing objects"));
            continue;
        }
        if !u.force && !current.is_zero() {
            let ff = repo.odb.read_commit(&new).is_ok() && is_ancestor(&repo.odb, &current, &new)?;
            if !ff {
                results.push(fail("non-fast-forward (pull first, or push --force)"));
                continue;
            }
        }
        repo.refs.write(&u.name, &new, &who, "push")?;
        results.push(RefResult { name: u.name.clone(), ok: true, message: "updated".into() });
    }
    Ok(results)
}

/// Copy one object as compressed bytes (loose-object file contents).
fn read_compressed(odb: &Odb, oid: &Oid) -> Result<Vec<u8>> {
    std::fs::read(odb.file_path(oid)).with_context(|| format!("reading object {oid}"))
}

/// Store compressed object bytes after verifying the hash.
fn store_compressed(odb: &Odb, oid: &Oid, data: &[u8]) -> Result<()> {
    if odb.exists(oid) {
        return Ok(());
    }
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(data).read_to_end(&mut raw)?;
    odb.write_raw(oid, &raw)
}

// ---------------------------------------------------------------- local transport

pub struct LocalRemote {
    pub repo: Repo,
}

impl Remote for LocalRemote {
    fn advertise(&mut self) -> Result<Advert> {
        advertise(&self.repo)
    }

    fn fetch(&mut self, wants: &[Oid], haves: &[Oid], odb: &Odb) -> Result<usize> {
        let objs = objects_to_send(&self.repo.odb, wants, haves)?;
        for o in &objs {
            store_compressed(odb, o, &read_compressed(&self.repo.odb, o)?)?;
        }
        Ok(objs.len())
    }

    fn push(&mut self, objects: &[Oid], local: &Odb, updates: &[RefUpdate]) -> Result<Vec<RefResult>> {
        for o in objects {
            store_compressed(&self.repo.odb, o, &read_compressed(local, o)?)?;
        }
        apply_updates(&self.repo, updates)
    }
}

// ---------------------------------------------------------------- framing

fn write_frame(w: &mut impl Write, data: &[u8]) -> Result<()> {
    w.write_all(&(data.len() as u32).to_be_bytes())?;
    w.write_all(data)?;
    Ok(())
}

fn read_frame(r: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

fn write_json<T: Serialize>(w: &mut impl Write, v: &T) -> Result<()> {
    write_frame(w, &serde_json::to_vec(v)?)
}

fn read_json(r: &mut impl Read) -> Result<serde_json::Value> {
    let f = read_frame(r)?.context("connection closed by remote")?;
    let v: serde_json::Value = serde_json::from_slice(&f).context("bad response from remote")?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        bail!("remote error: {e}");
    }
    Ok(v)
}

fn send_objects(w: &mut impl Write, odb: &Odb, objs: &[Oid]) -> Result<()> {
    for o in objs {
        let mut frame = o.0.to_vec();
        frame.extend_from_slice(&read_compressed(odb, o)?);
        write_frame(w, &frame)?;
    }
    write_frame(w, &[])
}

fn recv_objects(r: &mut impl Read, odb: &Odb) -> Result<usize> {
    let mut n = 0;
    loop {
        let f = read_frame(r)?.context("connection closed while receiving objects")?;
        if f.is_empty() {
            return Ok(n);
        }
        if f.len() < 20 {
            bail!("malformed object frame");
        }
        store_compressed(odb, &Oid::from_bytes(&f[..20])?, &f[20..])?;
        n += 1;
    }
}

// ---------------------------------------------------------------- SSH client

pub struct SshRemote {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl SshRemote {
    fn spawn(ssh: &str, host: &str, port: Option<u16>, remote_cmd: &str) -> Result<SshRemote> {
        let mut parts = ssh.split_whitespace();
        let prog = parts.next().unwrap_or("ssh");
        let mut cmd = Command::new(prog);
        cmd.args(parts);
        if let Some(p) = port {
            cmd.arg("-p").arg(p.to_string());
        }
        cmd.arg(host).arg(remote_cmd);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = cmd.spawn().with_context(|| format!("running '{prog}' (is OpenSSH installed?)"))?;
        let stdin = BufWriter::new(child.stdin.take().unwrap());
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(SshRemote { child, stdin, stdout })
    }

    fn request(&mut self, v: serde_json::Value) -> Result<()> {
        write_json(&mut self.stdin, &v)?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for SshRemote {
    fn drop(&mut self) {
        let _ = write_json(&mut self.stdin, &serde_json::json!({ "cmd": "quit" }));
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

impl Remote for SshRemote {
    fn advertise(&mut self) -> Result<Advert> {
        self.request(serde_json::json!({ "cmd": "ls-refs" }))?;
        Ok(serde_json::from_value(read_json(&mut self.stdout)?)?)
    }

    fn fetch(&mut self, wants: &[Oid], haves: &[Oid], odb: &Odb) -> Result<usize> {
        self.request(serde_json::json!({ "cmd": "fetch", "wants": wants, "haves": haves }))?;
        read_json(&mut self.stdout)?; // ack (or error)
        recv_objects(&mut self.stdout, odb)
    }

    fn push(&mut self, objects: &[Oid], local: &Odb, updates: &[RefUpdate]) -> Result<Vec<RefResult>> {
        self.request(serde_json::json!({ "cmd": "push", "updates": updates }))?;
        send_objects(&mut self.stdin, local, objects)?;
        self.stdin.flush()?;
        Ok(serde_json::from_value(read_json(&mut self.stdout)?["results"].clone())?)
    }
}

// ---------------------------------------------------------------- SSH server

/// `jg serve <path>`: speak the protocol on stdin/stdout. Run by SSH on the server.
pub fn serve(path: &std::path::Path) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut r = BufReader::new(stdin.lock());
    let mut w = BufWriter::new(stdout.lock());
    let repo = match Repo::open(path) {
        Ok(r) => r,
        Err(e) => {
            // Report the error to the client on its first request.
            let _ = read_frame(&mut r);
            write_json(&mut w, &serde_json::json!({ "error": format!("{e:#}") }))?;
            w.flush()?;
            return Ok(());
        }
    };
    while let Some(frame) = read_frame(&mut r)? {
        let req: serde_json::Value = serde_json::from_slice(&frame)?;
        let result: Result<()> = (|| {
            match req["cmd"].as_str().unwrap_or("") {
                "ls-refs" => write_json(&mut w, &advertise(&repo)?)?,
                "fetch" => {
                    let wants: Vec<Oid> = parse_oids(&req["wants"])?;
                    let haves: Vec<Oid> = parse_oids(&req["haves"])?;
                    let objs = objects_to_send(&repo.odb, &wants, &haves)?;
                    write_json(&mut w, &serde_json::json!({ "count": objs.len() }))?;
                    send_objects(&mut w, &repo.odb, &objs)?;
                }
                "push" => {
                    let updates: Vec<RefUpdate> = serde_json::from_value(req["updates"].clone())?;
                    recv_objects(&mut r, &repo.odb)?;
                    let results = apply_updates(&repo, &updates)?;
                    write_json(&mut w, &serde_json::json!({ "results": results }))?;
                }
                "quit" => return Err(anyhow::anyhow!("__quit__")),
                other => bail!("unknown command '{other}'"),
            }
            Ok(())
        })();
        match result {
            Ok(()) => {}
            Err(e) if e.to_string() == "__quit__" => break,
            Err(e) => write_json(&mut w, &serde_json::json!({ "error": format!("{e:#}") }))?,
        }
        w.flush()?;
    }
    Ok(())
}

fn parse_oids(v: &serde_json::Value) -> Result<Vec<Oid>> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(Oid::from_hex).collect()).unwrap_or(Ok(vec![]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_forms() {
        assert_eq!(
            parse_url("me@box:repos/x.jg"),
            Url::Ssh { host: "me@box".into(), port: None, path: "repos/x.jg".into() }
        );
        assert_eq!(
            parse_url("ssh://me@box:2222/srv/x"),
            Url::Ssh { host: "me@box".into(), port: Some(2222), path: "/srv/x".into() }
        );
        assert_eq!(parse_url("/srv/x"), Url::Local("/srv/x".into()));
        assert_eq!(parse_url("C:\\repos\\x"), Url::Local("C:\\repos\\x".into()));
        assert_eq!(parse_url("file:///srv/x"), Url::Local("/srv/x".into()));
    }
}
