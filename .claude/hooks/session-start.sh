#!/bin/bash
# Prepares a Claude Code cloud session: fetches the original/ submodule,
# installs Bevy's Linux build dependencies and pre-builds the workspace so the
# first check in a session is incremental. Safe to run repeatedly.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "$CLAUDE_PROJECT_DIR"

# Original C source and game data (behavioural reference, parser test data).
git submodule update --init --recursive

# Bevy's Linux build dependencies: ALSA (audio), udev (gamepads),
# Wayland and xkbcommon (windowing).
packages=(libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev)
missing=()
for package in "${packages[@]}"; do
  dpkg -s "$package" >/dev/null 2>&1 || missing+=("$package")
done
if [ ${#missing[@]} -gt 0 ]; then
  sudo=""
  [ "$(id -u)" -ne 0 ] && sudo="sudo"
  $sudo apt-get update -qq
  DEBIAN_FRONTEND=noninteractive $sudo apt-get install -y -qq --no-install-recommends "${missing[@]}"
fi

rustup component add clippy rustfmt >/dev/null 2>&1

# Warm the build cache; the container state is cached after this hook runs.
cargo clippy --workspace --all-targets --quiet
cargo test --workspace --no-run --quiet
