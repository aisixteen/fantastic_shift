#!/bin/sh
# remote-logs.sh <name> [-f] [-n LINES] [--dry-run] — the shift's log on the box.
# -f follows it; Ctrl-C stops following (the shift keeps running). Needs no keys.
set -eu
PROG=remote-logs
. "$(dirname "$0")/remote-common.sh"
USAGE='usage: remote-logs.sh <name> [-f] [-n LINES] [--dry-run]'

name= follow=0 lines=100
while [ $# -gt 0 ]; do
    case $1 in
        -f) follow=1; shift ;;
        -n) [ $# -ge 2 ] || die "-n needs a number"; lines=$2; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        -h | --help) echo "$USAGE"; exit 0 ;;
        -*) die "unknown flag \`$1\` — $USAGE" ;;
        *) [ -z "$name" ] || die "unexpected argument \`$1\` — $USAGE"; name=$1; shift ;;
    esac
done
[ -n "$name" ] || die "$USAGE"
case $lines in '' | *[!0-9]*) die "-n takes a number, got \`$lines\`" ;; esac
load_remote "$name"

# Runs on the box: $1 dir, $2 lines, $3 follow (1 or 0).
LOGS='[ -f "$1/logs/kernel.log" ] || { echo "no log yet: $1/logs/kernel.log" >&2; exit 4; }
if [ "$3" = 1 ]; then exec tail -n "$2" -f "$1/logs/kernel.log"; fi
exec tail -n "$2" "$1/logs/kernel.log"'

cmd=$(remote_cmd "$LOGS" "$R_DIR" "$lines" "$follow")
run ssh $FT_SSH_OPTS "$R_HOST" "$cmd" </dev/null
