#!/usr/bin/env bash
# Exercise the CI PR change-tour path against a throwaway fixture repository.
#
# The script creates a deterministic "pull request" (a base commit and a head
# commit), renders the change tour as markdown with --offline, and asserts that
# the output is stable and redacted. It is run by the `pr-tour` CI job.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

fixture="$repo_root/crates/laudit-core/tests/fixtures/git-repo"
repo="$workdir/fixture-pr"
mkdir -p "$repo"
cp -R "$fixture/." "$repo/"

export GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME="laudit ci"
export GIT_AUTHOR_EMAIL="ci@example.com"
export GIT_COMMITTER_NAME="laudit ci"
export GIT_COMMITTER_EMAIL="ci@example.com"

git -C "$repo" init -q
git -C "$repo" config user.name "laudit ci"
git -C "$repo" config user.email "ci@example.com"
git -C "$repo" add .
GIT_AUTHOR_DATE="2020-01-01T00:00:00Z" GIT_COMMITTER_DATE="2020-01-01T00:00:00Z" \
    git -C "$repo" -c commit.gpgsign=false commit -q -m "base"
base="$(git -C "$repo" rev-parse HEAD)"

cat >>"$repo/src/lib.rs" <<'EOF'

pub fn multiply(left: i32, right: i32) -> i32 {
    left * right
}

pub fn token() -> &'static str {
    "ghp_0123456789abcdefghijklmnopqrstuvwxyz"
}
EOF
git -C "$repo" add src/lib.rs
GIT_AUTHOR_DATE="2021-01-01T00:00:00Z" GIT_COMMITTER_DATE="2021-01-01T00:00:00Z" \
    git -C "$repo" -c commit.gpgsign=false commit -q -m "add multiply and token"
head="$(git -C "$repo" rev-parse HEAD)"

run_tour() {
    local out="$1"
    cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -p laudit-cli -- \
        --repo "$repo" tour --range "$base..$head" --md --offline --out "$out"
}

run_tour "$repo/tour.md"
run_tour "$repo/tour-again.md"

test -s "$repo/tour.md"
diff -u "$repo/tour.md" "$repo/tour-again.md"

grep -q "## Changes" "$repo/tour.md"
grep -q "multiply" "$repo/tour.md"
grep -q "«github_token#1»" "$repo/tour.md"

if grep -q "ghp_0123456789abcdefghijklmnopqrstuvwxyz" "$repo/tour.md"; then
    echo "error: secret leaked into the change tour" >&2
    exit 1
fi

echo "PR change tour fixture passed: $repo/tour.md"
