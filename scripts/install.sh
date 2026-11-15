#!/usr/bin/env bash
# Build jg from this source tree and install it for the current user (no root needed).
#
#   scripts/install.sh              # installs to ~/.local/bin/jg
#   scripts/install.sh /usr/local   # installs to /usr/local/bin/jg (may need sudo)
#
# For managed installs/updates on Ubuntu, prefer the .deb / APT repo (docs/RELEASING.md).
set -euo pipefail
cd "$(dirname "$0")/.."
source "$HOME/.cargo/env" 2>/dev/null || true
PREFIX=${1:-$HOME/.local}

cargo build --release --quiet
install -Dm755 target/release/jg "$PREFIX/bin/jg"
echo "Installed $("$PREFIX/bin/jg" version) to $PREFIX/bin/jg"

case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "NOTE: add $PREFIX/bin to your PATH, e.g.: echo 'export PATH=\"$PREFIX/bin:\$PATH\"' >> ~/.bashrc" ;;
esac
cat <<MSG

Next steps:
  jg config --global user.name  "Your Name"
  jg config --global user.email "you@example.com"
  # optional shell shortcuts (gs, ga, gc, gco, ...):
  echo 'eval "\$(jg shell-init bash)"' >> ~/.bashrc
MSG
