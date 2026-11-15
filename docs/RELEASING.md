# Building, packaging and releasing

## Toolchain

| Need | Install |
|---|---|
| Rust (stable, ≥ 1.88; developed on 1.100) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| C linker | `sudo apt install build-essential` |
| `.deb` builder | `cargo install cargo-deb` |
| APT repo metadata | `dpkg-scanpackages` (package `dpkg-dev`) |
| Windows cross-compile (optional) | `sudo apt install mingw-w64 && rustup target add x86_64-pc-windows-gnu` |

## Cutting a release

1. Bump `version` in `Cargo.toml` and add an entry to `CHANGELOG.md`.
2. Run:
   ```bash
   JG_RELEASE_NOTES="Short notes shown by self-update --check" scripts/release.sh
   ```
   This runs the tests and builds everything into `dist/`:
   ```
   dist/latest.json                    self-update manifest (version + sha256 per platform)
   dist/jg-<v>-linux-x86_64            raw Linux binary
   dist/jg-<v>-windows-x86_64.exe      raw Windows binary (if mingw is installed)
   dist/jg-<v>-windows-x86_64.zip      jg.exe + install.ps1 + README
   dist/apt/                           flat APT repository: jg_<v>-1_amd64.deb, Packages(.gz), Release
   ```
3. Publish: copy `dist/` to the update location, for example
   `rsync -av dist/ me@myserver:/var/www/jg/`. Any static web server works, or a
   shared folder.

## Update channels

- **Ubuntu (APT):** clients add `deb [trusted=yes] https://myserver/jg/apt ./` once.
  After that, `sudo apt update && sudo apt upgrade` picks up new releases.
  - To drop `[trusted=yes]`, sign the repo: `JG_GPG_KEY=<keyid> scripts/release.sh`,
    then on clients install the public key to `/etc/apt/keyrings/jg.gpg` and use
    `deb [signed-by=/etc/apt/keyrings/jg.gpg] https://myserver/jg/apt ./`.
- **Windows / non-apt installs:** `jg self-update` downloads `latest.json` from
  `update.source`, compares versions, downloads the platform binary with `curl`,
  checks its SHA-256, and swaps the executable. On Windows the running exe is renamed
  to `jg.exe.old` and deleted on the next run.
  - `jg self-update` refuses to replace binaries under `/usr/` (APT owns those) unless `--force`.

## Windows notes

- The release script cross-compiles with the GNU toolchain (`x86_64-pc-windows-gnu`).
  If you build on Windows with MSVC instead (`cargo build --release`), the output is
  `target\release\jg.exe`. Put it next to `packaging\windows\install.ps1` and run that script.
- Check that the code compiles for Windows without the linker: `cargo check --target x86_64-pc-windows-gnu`.
- Symlinks are stored as small files containing the link target on Windows (unless the
  process can create symlinks). The executable bit is kept from the index (`core.filemode` is ignored).

## Installing on a server for SSH remotes

```bash
sudo apt install jg            # from your APT repo, or: scripts/install.sh
jg init --bare ~/repos/project.jg
```
SSH runs `jg serve <path>` non-interactively, so `jg` must be on the non-login `PATH`.
`/usr/bin` works. For `~/.local/bin`, set `remote.<name>.jgpath` on clients.
