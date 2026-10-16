#!/usr/bin/env bash
# Rebase the agent-layer fork onto upstream/main and prove it still works.
# Usage: agent-layer/rebase.sh [upstream-branch]   (default: main)
#        agent-layer/rebase.sh --check             (only the checks, e.g. after resolving a conflict)
#        agent-layer/rebase.sh --full …            (the checks are `cargo xtask ci`, the whole gate)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
check_only=false
full=false
while [ $# -gt 0 ]; do
  case "$1" in
    --check) check_only=true; shift ;;
    --full) full=true; shift ;;
    *) break ;;
  esac
done
branch="${1:-main}"

markers() { git grep -c "fork:agent" -- ':!FORK.md' ':!agent-layer' ':!crates/agent' ':!crates/agent-ui' ':!apps/wordcraft/src/agent.rs' | awk -F: '{s+=$2} END {print s+0}'; }

if ! $check_only; then
  if [ -n "$(git status --porcelain)" ]; then
    echo "Working tree not clean: commit or stash first." >&2
    exit 1
  fi
  before=$(markers)
  git fetch upstream
  echo "Rebasing $(git rev-parse --abbrev-ref HEAD) onto upstream/$branch…"
  # Cargo.lock conflicts on nearly every rebase (upstream bumps dependencies): take upstream's lock
  # and let cargo add our two crates to it, without updating anything else.
  resolve_lock() {
    local files
    files=$(git diff --name-only --diff-filter=U)
    [ "$files" = "Cargo.lock" ] || return 1
    git checkout --ours Cargo.lock   # during a rebase, "ours" is upstream
    cargo metadata --format-version 1 >/dev/null
    git add Cargo.lock
    GIT_EDITOR=true git rebase --continue || true   # a later commit may stop again: the loop sees it
  }
  rebased=true
  git rebase "upstream/$branch" || rebased=false
  while ! $rebased; do
    if resolve_lock; then
      rebased=true
      [ -d "$(git rev-parse --git-path rebase-merge)" ] && rebased=false
    else
      break
    fi
  done
  if ! $rebased; then
    cat >&2 <<'MSG'

Conflict. It can only be in a line marked `fork:agent` (see FORK.md, "The patch to upstream files").
Keep upstream's version of the surrounding code, put the marked line back, then:
  git add <file> && git rebase --continue && agent-layer/rebase.sh --check
MSG
    exit 1
  fi
  after=$(markers)
  if [ "$after" -lt "$before" ]; then
    echo "Only $after of $before fork:agent markers left after the rebase: a hook line was lost." >&2
    exit 1
  fi
fi

echo "fork:agent markers in upstream files: $(markers)"
if $full; then
  cargo xtask ci
else
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test -p wordcraft-agent -p wordcraft-agent-ui -p wordcraft-engine -p wordcraft-mcp
  cargo test -p wordcraft-ui-egui --lib
  cargo xtask assets
  cargo xtask layers
  cargo xtask wasm
fi
cargo build --release -p wordcraft
echo "Rebased and checked: all gates green."
