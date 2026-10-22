#!/bin/sh
# check-secrets.sh [DIR] — fail when a tracked or new (not ignored) file under
# the git work tree DIR (default: .) holds a private key or a forge / API token.
# Exit 0: clean. 1: a hit (file:line and the kind are printed, never the match).
# 2: DIR is not a git work tree.
# Each pattern needs the token's body, not only its prefix, so a text that
# NAMES a prefix (like this one) is not a hit.
set -u
DIR=${1:-.}
if ! git -C "$DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "check-secrets: $DIR is not a git work tree" >&2
  exit 2
fi
found=0
check() {
  hits=$(cd "$DIR" && git ls-files -z -co --exclude-standard | xargs -0 grep -HnIE -e "$2" -- 2>/dev/null | cut -d: -f1,2)
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits" | while IFS= read -r h; do echo "check-secrets: $1 in $h" >&2; done
    found=1
  fi
}
check "private key" '-----BEGIN [A-Z ]*PRIVATE KEY'
check "GitHub token" 'gh[pousr]_[A-Za-z0-9]{36}'
check "GitHub fine-grained token" 'github_pat_[A-Za-z0-9_]{22,}'
check "Anthropic key" 'sk-ant-[A-Za-z0-9_-]{20,}'
check "token in a URL" 'x-access-token:[^@[:space:]$]{8,}@'
if [ "$found" -ne 0 ]; then
  echo "check-secrets: remove the secret, and rotate it if it was ever pushed" >&2
  exit 1
fi
exit 0
