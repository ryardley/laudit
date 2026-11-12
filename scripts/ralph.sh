#!/usr/bin/env bash
# ralph.sh: work through tk tickets one at a time.
#
# For each ready (non-epic) ticket it:
#   1. `tk start <id>`
#   2. `opencode run` (implementer)  ->  fresh session
#   3. `opencode run` (verifier)     ->  fresh session, must end VERDICT: PASS
#   4. on pass: `tk close <id>` and `git commit -m "<type>: <title>"`
#
# On FAIL it retries up to RALPH_MAX_ATTEMPTS, feeding the verifier's reason
# into the next implementer run. Exhausted attempts leave the ticket
# `in_progress` and abort the run.
#
# Needs `tk` (provided by devenv) and `opencode`. Run this inside the devenv
# shell (`devenv shell` or direnv).
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

TIMEOUT="${RALPH_TIMEOUT:-1800}" # seconds per opencode call (default 30 minutes)

usage() {
  cat <<'EOF'
usage: ralph.sh [--dry-run] [ticket-id]

Ralph loop over tk tickets. Without a ticket-id it processes `tk ready`
(non-epic) tickets one at a time; with a ticket-id it processes just that one
(it must exist, not be closed or an epic, and have its deps closed).

For each ticket:
  tk start <id>
  opencode run  # implement
  opencode run  # verify (must end "VERDICT: PASS")
  tk close <id> && git commit -m "<type>: <title>"

Env:
  RALPH_MAX_ATTEMPTS   implement+verify attempts per ticket (default 3)
  RALPH_MAX_ITERATIONS total tickets processed before stopping (default 100)
  RALPH_MODEL          model for opencode run (default: opencode default)
  RALPH_AGENT          agent for opencode run (default: opencode default)
  RALPH_TIMEOUT        seconds per opencode call (default 1800)

Logs: .ralph/logs/<ticket>-<attempt>-<implement|verify>.log
      .ralph/run-<timestamp>.log
EOF
}

die() { echo "error: $*" >&2; exit 1; }

ONLY=""
DRY_RUN=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) die "unknown option: $1 (see --help)" ;;
    *) ONLY="$1"; shift ;;
  esac
done

MAX_ATTEMPTS="${RALPH_MAX_ATTEMPTS:-3}"
MAX_ITERATIONS="${RALPH_MAX_ITERATIONS:-100}"
if ! [[ "$MAX_ATTEMPTS" =~ ^[0-9]+$ ]] || (( MAX_ATTEMPTS < 1 )); then
  die "RALPH_MAX_ATTEMPTS must be an integer >= 1"
fi
if ! [[ "$MAX_ITERATIONS" =~ ^[0-9]+$ ]] || (( MAX_ITERATIONS < 1 )); then
  die "RALPH_MAX_ITERATIONS must be an integer >= 1"
fi
if ! [[ "$TIMEOUT" =~ ^[0-9]+$ ]] || (( TIMEOUT < 1 )); then
  die "RALPH_TIMEOUT must be an integer >= 1"
fi

for tool in tk opencode git timeout; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool not found on PATH: $tool"
done

export NO_COLOR=1

if (( ! DRY_RUN )); then
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "error: working tree is dirty; commit or stash changes first" >&2
    git status --short >&2
    exit 1
  fi
fi

ticket_type()   { tk show "$1" | sed -n 's/^type: *//p' | head -1; }
ticket_status() { tk show "$1" | sed -n 's/^status: *//p' | head -1; }
ticket_title()  { tk show "$1" | sed -n 's/^# //p' | head -1; }

commit_prefix() {
  case "$(ticket_type "$1")" in
    bug) echo "fix" ;;
    chore) echo "chore" ;;
    *) echo "feat" ;;
  esac
}

next_ready() {
  local line id
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    id="${line%% *}"
    if [[ "$(ticket_type "$id")" != "epic" ]]; then
      echo "$id"
      return 0
    fi
  done < <(tk ready)
  return 1
}

last_verdict() {
  local log="$1" line
  line=$(grep -E 'VERDICT: (PASS|FAIL)' "$log" 2>/dev/null | tail -1)
  if [[ "$line" == *"VERDICT: PASS"* ]]; then echo "PASS"; else echo "FAIL"; fi
}

verdict_reason() {
  local log="$1" line
  line=$(grep -E 'VERDICT: FAIL' "$log" 2>/dev/null | tail -1)
  if [[ -n "$line" ]]; then
    echo "$line"
    sed -n '/VERDICT: FAIL/,$p' "$log" | tail -30
  else
    tail -30 "$log"
  fi
}

run_opencode() {
  local role="$1" prompt="$2" log="$3" id="$4" attempt="$5"
  local args=(run --auto)
  [[ -n "${RALPH_MODEL:-}" ]] && args+=(--model "$RALPH_MODEL")
  [[ -n "${RALPH_AGENT:-}" ]] && args+=(--agent "$RALPH_AGENT")
  args+=(--title "ralph: $id-$attempt-$role")
  timeout --kill-after=30 "$TIMEOUT" opencode "${args[@]}" "$prompt" 2>&1 | tee "$log"
  return "${PIPESTATUS[0]}"
}

implementer_prompt() {
  local id="$1" title="$2" attempt="$3" reason="$4"
  cat <<EOF
You are implementing tk ticket $id: $title in this repository.

Steps:
1. Read docs/architecture.md and docs/decisions.md for orientation. These are authoritative; if the ticket contradicts them, follow the docs and note the discrepancy.
2. Run \`tk show $id\` (or read .tickets/$id.md) for the full ticket: Description, Design, Acceptance Criteria, References. Read the documents the References point to.
3. Implement per the Design to satisfy every item in the Acceptance Criteria.
4. Run the relevant build/tests to confirm your work: \`cargo fmt\`, \`cargo fmt --check\`, \`cargo clippy --workspace --all-targets -- -D warnings\`, and \`cargo test --workspace\` (plus any ticket-specific checks).
5. Update specs/docs only where the ticket explicitly requires it.
6. Do NOT run tk status commands (start/close/status) and do NOT git commit. The orchestrator handles those.
7. Work only inside the repository root: $ROOT

Verifier feedback from the last attempt:
$([ "$attempt" -gt 1 ] && printf '%s' "$reason" || printf 'none (first attempt)')
EOF
}

verifier_prompt() {
  local id="$1" title="$2"
  cat <<EOF
Independently verify that tk ticket $id: $title is complete in this repository.

Steps:
1. Read docs/architecture.md and docs/decisions.md. These are authoritative.
2. Run \`tk show $id\` to read its Acceptance Criteria and References; read the referenced docs.
3. Inspect the working tree / git diff and the relevant code; check each acceptance criterion.
4. Run the relevant build/tests: \`cargo fmt --check\`, \`cargo clippy --workspace --all-targets -- -D warnings\`, \`cargo test --workspace\`, plus any ticket-specific checks.
5. Do NOT modify any files, do NOT run tk status commands, do NOT git commit.
6. End your final message with exactly one line of the form:
   VERDICT: <result>
   where <result> is PASS if all acceptance criteria are met, or
   FAIL: <reason> otherwise.
EOF
}

process_ticket() {
  local id="$1" attempt rc iprompt vprompt ilog vlog verdict reason pass
  local title
  title=$(ticket_title "$id")

  if (( DRY_RUN )); then
    echo "  (dry-run) would: tk start $id; opencode implement; opencode verify; tk close; git commit -m \"$(commit_prefix "$id"): $title\""
    return 0
  fi

  echo "== start $id ($title)"
  tk start "$id" >/dev/null

  pass=0
  reason=""
  for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
    ilog="$LOGDIR/$id-$attempt-implement.log"
    vlog="$LOGDIR/$id-$attempt-verify.log"

    echo "  [$id] attempt $attempt/$MAX_ATTEMPTS: implement (log: $ilog)"
    iprompt=$(implementer_prompt "$id" "$title" "$attempt" "$reason")
    set +e
    run_opencode implement "$iprompt" "$ilog" "$id" "$attempt"
    rc=$?
    set -e
    if (( rc != 0 )); then
      echo "  [$id] implementer exited non-zero/timeout (rc=$rc); see $ilog"
      reason="implementer process exited non-zero (rc=$rc) or timed out; see $ilog"
      continue
    fi

    echo "  [$id] attempt $attempt/$MAX_ATTEMPTS: verify (log: $vlog)"
    vprompt=$(verifier_prompt "$id" "$title")
    set +e
    run_opencode verify "$vprompt" "$vlog" "$id" "$attempt"
    rc=$?
    set -e
    if (( rc != 0 )); then
      echo "  [$id] verifier exited non-zero/timeout (rc=$rc); treating as FAIL"
    fi

    verdict=$(last_verdict "$vlog")
    if [[ "$verdict" == "PASS" ]]; then
      pass=1
      break
    fi
    reason=$(verdict_reason "$vlog")
    echo "  [$id] verification FAILED: $reason"
  done

  if (( pass )); then
    echo "  [$id] PASS"
    tk close "$id" >/dev/null
    git add -A
    if git diff --cached --quiet; then
      echo "  [$id] closed (no file changes to commit)"
    else
      local msg
      msg="$(commit_prefix "$id"): $title"
      git commit -q -m "$msg"
      echo "  [$id] closed and committed: $msg"
    fi
    CLOSED_COUNT=$((CLOSED_COUNT + 1))
  else
    echo "  [$id] FAILED after $MAX_ATTEMPTS attempts; left in_progress" >&2
    return 1
  fi
}

# ---- positional ticket validation ----
if [[ -n "$ONLY" ]]; then
  if ! tk show "$ONLY" >/dev/null 2>&1; then
    die "ticket '$ONLY' not found"
  fi
  ONLY=$(tk show "$ONLY" | sed -n 's/^id: *//p' | head -1)
  [[ -n "$ONLY" ]] || die "could not resolve ticket id"
  if [[ "$(ticket_type "$ONLY")" == "epic" ]]; then
    die "$ONLY is an epic; run it via its children instead"
  fi
  if [[ "$(ticket_status "$ONLY")" == "closed" ]]; then
    die "$ONLY is already closed"
  fi
  if ! tk ready | awk '{print $1}' | grep -qx "$ONLY"; then
    die "$ONLY is not ready (deps unresolved); run 'tk show $ONLY' to see blockers"
  fi
fi

# ---- dry-run: just print the plan ----
if (( DRY_RUN )); then
  echo "Dry run. Would process these ready (non-epic) tickets:"
  n=0
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    id="${line%% *}"
    if [[ "$(ticket_type "$id")" != "epic" ]]; then
      if [[ -n "$ONLY" && "$id" != "$ONLY" ]]; then continue; fi
      n=$((n + 1))
      echo "  $n. $id - $(ticket_title "$id")"
    fi
  done < <(tk ready)
  echo "(each: tk start -> opencode implement -> opencode verify -> tk close -> git commit 'feat: <title>')"
  exit 0
fi

LOGDIR="$ROOT/.ralph/logs"
mkdir -p "$LOGDIR"
RUNLOG="$ROOT/.ralph/run-$(date +%Y%m%d-%H%M%S).log"
exec > >(tee -a "$RUNLOG") 2>&1

# ---- main loop ----
CLOSED_COUNT=0
ITERATIONS=0
ONLY_DONE=0
while :; do
  if [[ -n "$ONLY" ]]; then
    if (( ONLY_DONE )); then break; fi
    id="$ONLY"
    ONLY_DONE=1
  else
    if ! id=$(next_ready); then
      echo "No ready (non-epic) tickets left after $ITERATIONS iteration(s)."
      break
    fi
  fi

  ITERATIONS=$((ITERATIONS + 1))
  if (( ITERATIONS > MAX_ITERATIONS )); then
    die "exceeded RALPH_MAX_ITERATIONS ($MAX_ITERATIONS)"
  fi

  echo "=== [$ITERATIONS] $id - $(ticket_title "$id")"
  if ! process_ticket "$id"; then
    exit 1
  fi
done

# ---- closing summary ----
echo ""
if [[ -z "$ONLY" ]]; then
  blocked=$(tk blocked || true)
  if [[ -n "$blocked" ]]; then
    echo "Closed $CLOSED_COUNT ticket(s). Blocked tickets remain (deps unresolved):"
    echo "$blocked"
    exit 1
  fi
fi
echo "Done. Closed $CLOSED_COUNT ticket(s)."
exit 0
