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
# Wayland and xkbcommon (windowing). The rest let agents run the game and
# viewer headlessly for screenshots: a virtual X display, X11 keyboard support
# and Mesa's software Vulkan driver.
packages=(libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev
  xvfb libxkbcommon-x11-0 mesa-vulkan-drivers)
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
# Clippy warms the check cache. The test binaries are built too: a cold
# codegen build of Bevy takes about 30 minutes, and agents working in
# parallel worktrees share this target directory, so it is paid only once.
cargo clippy --workspace --all-targets --quiet
cargo test --workspace --no-run --quiet
