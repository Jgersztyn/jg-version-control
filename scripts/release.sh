#!/usr/bin/env bash
# Build a jg release into ./dist — a folder you can copy to any web server or share.
#
#   scripts/release.sh            # Linux binary + .deb + APT repo + latest.json
#   (Windows .exe is added automatically when the mingw toolchain is installed)
#
# Output layout (publish the whole dist/ folder at your update.source location):
#   dist/latest.json                         <- read by `jg self-update`
#   dist/jg-<ver>-linux-x86_64               <- raw binary for self-update
#   dist/jg-<ver>-windows-x86_64.exe         <- raw binary for self-update
#   dist/jg-<ver>-windows-x86_64.zip         <- jg.exe + install.ps1 for first install
#   dist/apt/                                <- flat APT repository (Packages, Release, .deb)
#
# Optional: JG_GPG_KEY=<key id> signs the APT Release file (then drop [trusted=yes]).
set -euo pipefail
cd "$(dirname "$0")/.."
source "$HOME/.cargo/env" 2>/dev/null || true

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
DIST=dist
NOTES=${JG_RELEASE_NOTES:-}
echo "==> Building jg $VERSION"

cargo test --quiet
cargo build --release --quiet

# Shell completions (packaged into the .deb)
mkdir -p target/completions
./target/release/jg completions bash > target/completions/jg.bash
./target/release/jg completions zsh  > target/completions/_jg
./target/release/jg completions fish > target/completions/jg.fish

rm -rf "$DIST" && mkdir -p "$DIST/apt"
cp target/release/jg "$DIST/jg-$VERSION-linux-x86_64"

# Debian package
cargo deb --no-build --output "$DIST/apt/" >/dev/null
echo "==> Built $(ls "$DIST"/apt/*.deb)"

# Windows (cross-compiled) if the toolchain is present
WIN_JSON=""
if rustup target list --installed | grep -q x86_64-pc-windows-gnu && command -v x86_64-w64-mingw32-gcc >/dev/null; then
  cargo build --release --quiet --target x86_64-pc-windows-gnu
  WIN_EXE="$DIST/jg-$VERSION-windows-x86_64.exe"
  cp target/x86_64-pc-windows-gnu/release/jg.exe "$WIN_EXE"
  STAGE=$(mktemp -d)
  cp target/x86_64-pc-windows-gnu/release/jg.exe packaging/windows/install.ps1 "$STAGE/"
  cp README.md "$STAGE/README.md"
  (cd "$STAGE" && zip -q "$OLDPWD/$DIST/jg-$VERSION-windows-x86_64.zip" jg.exe install.ps1 README.md)
  rm -rf "$STAGE"
  WIN_SHA=$(sha256sum "$WIN_EXE" | cut -d' ' -f1)
  WIN_JSON=",
    \"windows-x86_64\": { \"file\": \"jg-$VERSION-windows-x86_64.exe\", \"sha256\": \"$WIN_SHA\" }"
  echo "==> Built Windows binary"
else
  echo "==> Skipping Windows build (install: sudo apt install mingw-w64 && rustup target add x86_64-pc-windows-gnu)"
fi

# APT repository metadata (flat repo)
(
  cd "$DIST/apt"
  dpkg-scanpackages --multiversion . /dev/null > Packages 2>/dev/null
  gzip -9kf Packages
  {
    echo "Origin: jg"
    echo "Label: jg"
    echo "Suite: stable"
    echo "Codename: stable"
    echo "Architectures: amd64"
    echo "Date: $(date -Ru)"
    echo "SHA256:"
    for f in Packages Packages.gz; do
      echo " $(sha256sum "$f" | cut -d' ' -f1) $(stat -c%s "$f") $f"
    done
  } > Release
  if [ -n "${JG_GPG_KEY:-}" ]; then
    gpg --default-key "$JG_GPG_KEY" -abs -o Release.gpg Release
    gpg --default-key "$JG_GPG_KEY" --clearsign -o InRelease Release
  fi
)

# Self-update manifest
LIN_SHA=$(sha256sum "$DIST/jg-$VERSION-linux-x86_64" | cut -d' ' -f1)
cat > "$DIST/latest.json" <<JSON
{
  "version": "$VERSION",
  "notes": $(printf '%s' "$NOTES" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'),
  "assets": {
    "linux-x86_64": { "file": "jg-$VERSION-linux-x86_64", "sha256": "$LIN_SHA" }$WIN_JSON
  }
}
JSON

echo "==> Release ready in $DIST/:"
ls -1 "$DIST" "$DIST/apt"
