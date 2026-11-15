//! `Repo`: an opened repository (its `.jg` directory plus optional work tree).

use super::config::{Config, ConfigFile};
use super::index::Index;
use super::object::{MODE_EXEC, MODE_FILE, MODE_SYMLINK, Signature};
use super::odb::Odb;
use super::oid::Oid;
use super::refs::{Head, Refs};
use super::tree::{self, FlatTree};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

pub const JG_DIR: &str = ".jg";
pub const DEFAULT_BRANCH: &str = "main";

pub struct Repo {
    /// Work tree root; `None` for bare repositories (e.g. on a server).
    pub work: Option<PathBuf>,
    /// The repository metadata directory (`.jg`, or the repo itself when bare).
    pub dir: PathBuf,
    pub odb: Odb,
    pub refs: Refs,
    pub config: Config,
    /// Lazily loaded `.gitattributes` rules (line-ending handling).
    pub attrs: std::cell::RefCell<super::attributes::Attributes>,
}

impl Repo {
    /// Create a new repository. `path` is the work tree (or the bare repo dir).
    pub fn init(path: &Path, bare: bool, branch: Option<&str>) -> Result<Repo> {
        let dir = if bare { path.to_path_buf() } else { path.join(JG_DIR) };
        if dir.join("HEAD").exists() {
            bail!("repository already exists at {}", dir.display());
        }
        fs::create_dir_all(dir.join("objects"))?;
        fs::create_dir_all(dir.join("refs").join("heads"))?;
        fs::create_dir_all(dir.join("refs").join("tags"))?;
        let global = Config::load(None);
        let branch = branch
            .map(String::from)
            .or_else(|| global.get("init.defaultBranch"))
            .unwrap_or_else(|| DEFAULT_BRANCH.to_string());
        super::refs::check_name(&branch)?;
        fs::write(dir.join("HEAD"), format!("ref: refs/heads/{branch}\n"))?;
        let mut cfg = ConfigFile::load(&dir.join("config"));
        cfg.set("core.repositoryformatversion", "0")?;
        cfg.set("core.bare", if bare { "true" } else { "false" })?;
        cfg.save()?;
        fs::write(dir.join("description"), "jg repository. Object format is git-compatible (SHA-1 loose objects).\n")?;
        Repo::open_dir(&dir, if bare { None } else { Some(path.to_path_buf()) })
    }

    fn open_dir(dir: &Path, work: Option<PathBuf>) -> Result<Repo> {
        let dir = dir.to_path_buf();
        Ok(Repo {
            work,
            odb: Odb::new(dir.join("objects")),
            refs: Refs::new(dir.clone()),
            config: Config::load(Some(&dir.join("config"))),
            attrs: Default::default(),
            dir,
        })
    }

    /// Open a repo at exactly `path` (work tree containing `.jg`, or a bare repo dir).
    pub fn open(path: &Path) -> Result<Repo> {
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if path.join(JG_DIR).join("HEAD").is_file() {
            return Repo::open_dir(&path.join(JG_DIR), Some(path));
        }
        if path.join("HEAD").is_file() && path.join("objects").is_dir() {
            return Repo::open_dir(&path, None);
        }
        bail!("not a jg repository: {}", path.display())
    }

    /// Find the repository containing `start` by walking up parent directories.
    pub fn discover(start: &Path) -> Result<Repo> {
        let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
        let mut cur = Some(start.as_path());
        while let Some(d) = cur {
            if d.join(JG_DIR).join("HEAD").is_file() {
                return Repo::open_dir(&d.join(JG_DIR), Some(d.to_path_buf()));
            }
            cur = d.parent();
        }
        if start.join("HEAD").is_file() && start.join("objects").is_dir() {
            return Repo::open_dir(&start, None);
        }
        bail!("not a jg repository (or any parent): {}\nhint: run 'jg init' to create one", start.display())
    }

    pub fn is_bare(&self) -> bool {
        self.work.is_none()
    }

    pub fn work_dir(&self) -> Result<&Path> {
        self.work.as_deref().context("this operation needs a work tree (repository is bare)")
    }

    pub fn reload_config(&mut self) {
        self.config = Config::load(Some(&self.dir.join("config")));
    }

    pub fn repo_config_file(&self) -> ConfigFile {
        ConfigFile::load(&self.dir.join("config"))
    }

    // ----- locking -----------------------------------------------------------

    /// Take the repository write lock. Released when the guard is dropped.
    /// Prevents two processes (e.g. a human and an AI agent) from mutating at once.
    pub fn lock(&self) -> Result<LockGuard> {
        let p = self.dir.join("jg.lock");
        match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => Ok(LockGuard { path: p }),
            Err(_) => bail!(
                "repository is locked by another jg process ({}).\nIf no other jg command is running, delete that file.",
                p.display()
            ),
        }
    }

    // ----- index -------------------------------------------------------------

    pub fn index_path(&self) -> PathBuf {
        self.dir.join("index")
    }

    pub fn load_index(&self) -> Result<Index> {
        Index::load(&self.index_path())
    }

    pub fn save_index(&self, idx: &mut Index) -> Result<()> {
        idx.save(&self.index_path())
    }

    // ----- identity ----------------------------------------------------------

    /// Author/committer identity from env (`JG_AUTHOR_NAME`/`JG_AUTHOR_EMAIL`) or config.
    pub fn signature(&self) -> Result<Signature> {
        let name = std::env::var("JG_AUTHOR_NAME").ok().or_else(|| self.config.get("user.name"));
        let email = std::env::var("JG_AUTHOR_EMAIL").ok().or_else(|| self.config.get("user.email"));
        match (name, email) {
            (Some(n), Some(e)) => {
                let mut sig = Signature::now(&n, &e);
                // Deterministic timestamps for tests/reproducible builds.
                if let Ok(t) = std::env::var("JG_DATE")
                    && let Some((secs, tz)) = t.split_once(' ')
                {
                    sig.time = secs.parse().unwrap_or(sig.time);
                    sig.tz_offset_min = crate::util::parse_tz(tz);
                }
                Ok(sig)
            }
            _ => bail!(
                "who are you? Set your identity first:\n  jg config --global user.name \"Your Name\"\n  jg config --global user.email \"you@example.com\""
            ),
        }
    }

    /// Identity string for reflog entries (never fails).
    pub fn who(&self) -> String {
        match self.signature() {
            Ok(s) => s.to_line(),
            Err(_) => {
                let (t, tz) = crate::util::now_with_offset();
                format!("unknown <unknown> {t} {}", crate::util::format_tz(tz))
            }
        }
    }

    // ----- HEAD helpers ------------------------------------------------------

    pub fn head(&self) -> Result<Head> {
        self.refs.read_head()
    }

    pub fn head_oid(&self) -> Result<Option<Oid>> {
        self.refs.read("HEAD")
    }

    /// Short name of the current branch, or None when detached.
    pub fn current_branch(&self) -> Result<Option<String>> {
        Ok(match self.head()? {
            Head::Branch(r) => Some(r.strip_prefix("refs/heads/").unwrap_or(&r).to_string()),
            Head::Detached(_) => None,
        })
    }

    pub fn commit_tree_map(&self, commit: &Oid) -> Result<FlatTree> {
        let c = self.odb.read_commit(commit)?;
        tree::flatten(&self.odb, &c.tree)
    }

    /// Flattened tree of HEAD (empty if there are no commits yet).
    pub fn head_tree_map(&self) -> Result<FlatTree> {
        match self.head_oid()? {
            Some(o) => self.commit_tree_map(&o),
            None => Ok(FlatTree::new()),
        }
    }

    // ----- work tree file I/O -------------------------------------------------

    pub fn work_path(&self, repo_path: &str) -> Result<PathBuf> {
        Ok(crate::util::from_repo_path(self.work_dir()?, repo_path))
    }

    /// Line-ending conversion for a path (see `core::attributes`).
    pub fn conversion(&self, repo_path: &str) -> super::attributes::Conversion {
        let Some(work) = self.work.as_deref() else { return super::attributes::Conversion::NONE };
        let autocrlf = self.config.get("core.autocrlf").unwrap_or_default().to_lowercase();
        let eol = self.config.get("core.eol").unwrap_or_default().to_lowercase();
        self.attrs.borrow_mut().conversion(work, repo_path, &autocrlf, &eol)
    }

    /// Read a work-tree file's content as git would store it (symlinks -> target path,
    /// line endings normalized per `.gitattributes` / `core.autocrlf`).
    pub fn read_work_file(&self, repo_path: &str) -> Result<Vec<u8>> {
        let p = self.work_path(repo_path)?;
        let meta = fs::symlink_metadata(&p)?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&p)?;
            return Ok(target.to_string_lossy().replace('\\', "/").into_bytes());
        }
        let data = fs::read(&p)?;
        Ok(super::attributes::clean(self.conversion(repo_path), data))
    }

    /// Determine the git mode for a work-tree file.
    pub fn work_file_mode(&self, meta: &fs::Metadata, previous: Option<u32>) -> u32 {
        if meta.file_type().is_symlink() {
            return MODE_SYMLINK;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if self.config.get_bool("core.filemode", true) {
                return if meta.permissions().mode() & 0o111 != 0 { MODE_EXEC } else { MODE_FILE };
            }
        }
        // Windows (or filemode=false): keep whatever mode the file had before.
        match previous {
            Some(m) if m == MODE_EXEC || m == MODE_FILE => m,
            _ => MODE_FILE,
        }
    }

    /// Write a blob into the work tree with the right mode.
    pub fn write_work_file(&self, repo_path: &str, mode: u32, oid: &Oid) -> Result<()> {
        let p = self.work_path(repo_path)?;
        let data = self.odb.read_blob(oid)?;
        if let Some(parent) = p.parent() {
            // A file may be sitting where a directory needs to go.
            if parent.is_file() {
                fs::remove_file(parent)?;
            }
            fs::create_dir_all(parent)?;
        }
        if fs::symlink_metadata(&p).is_ok() {
            if p.is_dir() && !fs::symlink_metadata(&p)?.file_type().is_symlink() {
                fs::remove_dir_all(&p)?;
            } else {
                fs::remove_file(&p)?;
            }
        }
        if mode == MODE_SYMLINK {
            let target = String::from_utf8_lossy(&data).into_owned();
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&target, &p)?;
                return Ok(());
            }
            #[cfg(not(unix))]
            {
                // Without symlink privileges on Windows, store the link target as a plain file.
                fs::write(&p, target.as_bytes())?;
                return Ok(());
            }
        }
        let data = super::attributes::smudge(self.conversion(repo_path), data);
        fs::write(&p, &data).with_context(|| format!("writing {}", p.display()))?;
        if repo_path.ends_with(".gitattributes") {
            self.attrs.borrow_mut().clear();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = if mode == MODE_EXEC { 0o755 } else { 0o644 };
            fs::set_permissions(&p, fs::Permissions::from_mode(perm))?;
        }
        Ok(())
    }

    pub fn remove_work_file(&self, repo_path: &str) -> Result<()> {
        let p = self.work_path(repo_path)?;
        if fs::symlink_metadata(&p).is_ok() {
            fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            crate::util::prune_empty_dirs(&p, self.work_dir()?);
        }
        Ok(())
    }

    /// Convert a user-supplied path (relative to cwd) into a repo path.
    pub fn to_repo_path(&self, user_path: &Path) -> Result<String> {
        let work = self.work_dir()?;
        let abs =
            if user_path.is_absolute() { user_path.to_path_buf() } else { std::env::current_dir()?.join(user_path) };
        let abs = crate::util::normalize(&abs);
        // Canonicalize the parent to resolve symlinked dirs like /tmp -> /private/tmp.
        let abs = match (abs.parent(), abs.file_name()) {
            (Some(par), Some(name)) => fs::canonicalize(par).map(|p| p.join(name)).unwrap_or(abs.clone()),
            _ => fs::canonicalize(&abs).unwrap_or(abs.clone()),
        };
        let rel =
            abs.strip_prefix(work).with_context(|| format!("'{}' is outside the repository", user_path.display()))?;
        Ok(crate::util::to_repo_path(rel))
    }

    /// Commit-producing state files used by merge/cherry-pick/revert.
    pub fn state_file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
