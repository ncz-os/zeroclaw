#!/usr/bin/env bash
# fork-reconcile.sh — watch which zoder patches upstream has adopted, and flag
# conflict risk, so the zoder-integration patch base can be reconciled against
# zeroclaw-labs/zeroclaw master. Read-only: it reports, it does not mutate.
#
# Usage:  scripts/fork-reconcile.sh
# Needs:  git remote `upstream` -> zeroclaw-labs/zeroclaw; `gh` authed; run from
#         a zoder-integration checkout with PATCH-STACK.md at the repo root.
set -euo pipefail

UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
UPSTREAM_REPO="${UPSTREAM_REPO:-zeroclaw-labs/zeroclaw}"
ROOT="$(git rev-parse --show-toplevel)"
MANIFEST="$ROOT/PATCH-STACK.md"
[ -f "$MANIFEST" ] || { echo "no PATCH-STACK.md at repo root"; exit 1; }

git remote get-url "$UPSTREAM_REMOTE" >/dev/null 2>&1 \
  || git remote add "$UPSTREAM_REMOTE" "https://github.com/${UPSTREAM_REPO}.git"
echo "fetching ${UPSTREAM_REMOTE}/master…" >&2
git fetch -q "$UPSTREAM_REMOTE" master
BASE="$(git merge-base HEAD "${UPSTREAM_REMOTE}/master")"
echo "merge-base with ${UPSTREAM_REMOTE}/master: ${BASE:0:9}" >&2

# Files upstream has changed since our merge-base (for conflict-risk on fork-owned paths).
UPSTREAM_CHANGED="$(git diff --name-only "$BASE" "${UPSTREAM_REMOTE}/master")"

adopted=0; drift=0; risk=0
printf '\n%-26s %-8s %-12s %s\n' "PATCH" "PR" "MANIFEST" "UPSTREAM / NOTES"
printf '%s\n' "--------------------------------------------------------------------------------"

# Parse manifest rows: | patch | paths | PR | status |
grep -E '^\| [a-z]' "$MANIFEST" | while IFS='|' read -r _ patch paths pr status _; do
  patch="$(echo "$patch" | xargs)"; pr="$(echo "$pr" | xargs)"
  status="$(echo "$status" | tr -d '*' | xargs)"; paths="$(echo "$paths" | xargs)"
  prnum="${pr#\#}"

  note=""
  if [[ "$prnum" =~ ^[0-9]+$ ]]; then
    state="$(gh pr view "$prnum" -R "$UPSTREAM_REPO" --json state,mergeCommit \
              --jq '.state + " " + (.mergeCommit.oid // "")' 2>/dev/null || echo "UNKNOWN")"
    pstate="${state%% *}"; psha="${state##* }"
    if [ "$pstate" = "MERGED" ]; then
      if [ "$status" != "adopted" ]; then
        note="*** NEWLY ADOPTED upstream (sha ${psha:0:9}) — reconcile: take upstream, mark adopted"; drift=$((drift+1))
      else
        note="adopted (already reconciled)"; adopted=$((adopted+1))
      fi
    else
      note="upstream state=${pstate}"
    fi
  else
    note="fork-only (no upstream PR)"
  fi

  # Conflict-risk: did upstream touch any path this patch owns?
  for p in $(echo "$paths" | tr ',' ' ' | tr -d '`'); do
    p="${p%%(*}"; p="$(echo "$p" | xargs)"; [ -z "$p" ] && continue
    if echo "$UPSTREAM_CHANGED" | grep -qF "$p"; then
      note="$note | UPSTREAM TOUCHED $p (conflict risk)"; risk=$((risk+1))
    fi
  done

  printf '%-26s %-8s %-12s %s\n' "$patch" "${pr:--}" "$status" "$note"
done

echo
echo "Run a reconcile when any row reports NEWLY ADOPTED or a conflict risk:"
echo "  git checkout -b integrate/master-sync-\$(date +%Y%m%d) && git merge ${UPSTREAM_REMOTE}/master"
echo "  # take upstream for adopted files; keep fork-only/open patches; regen Cargo.lock; cargo check; then advance zoder-integration"
