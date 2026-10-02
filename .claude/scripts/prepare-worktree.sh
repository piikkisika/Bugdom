#!/bin/bash
# Usage: .claude/scripts/prepare-worktree.sh <worktree>
#
# Prepares a git worktree of the Bugdom repo for building without a cold
# Bevy build: links the original/ submodule and seeds target/ with hard links
# to the main checkout's dependency artifacts. The workspace crates' own
# artifacts are left out, so they are always built from this worktree's
# sources and never shared between checkouts.
set -euo pipefail
wt="$(cd "$1" && pwd)"
main="$(git -C "$wt" worktree list --porcelain | head -1 | cut -d" " -f2-)"
if [ ! -e "$wt/original/Data" ]; then
  rmdir "$wt/original" 2>/dev/null || true
  ln -s "$main/original" "$wt/original"
fi
if [ ! -d "$wt/target" ]; then
  cp -al "$main/target" "$wt/target"
  for d in "$wt/target/debug" "$wt/target/debug/deps" "$wt/target/debug/.fingerprint" \
           "$wt/target/debug/incremental" "$wt/target/debug/build"; do
    rm -rf "$d"/bugdom* "$d"/libbugdom* "$d"/viewer* "$d"/bugdom_* 2>/dev/null || true
  done
fi
echo "prepared $wt"
