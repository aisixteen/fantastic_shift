#!/bin/sh
# remote-stop.sh <name> [--now] [--yes] [--dry-run]
# Asks first. By default it drains: SIGTERM, so running tasks finish or their
# leases return them to the queue. --now sends SIGKILL. --yes skips the question.
set -eu
PROG=remote-stop
. "$(dirname "$0")/remote-common.sh"
USAGE='usage: remote-stop.sh <name> [--now] [--yes] [--dry-run]'

name= now=0 yes=0
while [ $# -gt 0 ]; do
    case $1 in
        --now) now=1; shift ;;
        --yes) yes=1; shift ;;
        --dry-run) DRY=1; shift ;;
        -h | --help) echo "$USAGE"; exit 0 ;;
        -*) die "unknown flag \`$1\` — $USAGE" ;;
        *) [ -z "$name" ] || die "unexpected argument \`$1\` — $USAGE"; name=$1; shift ;;
    esac
done
[ -n "$name" ] || die "$USAGE"
load_remote "$name"

if [ "$now" = 1 ]; then
    sig=KILL
    question="stop $name on $R_HOST NOW? running tasks are cut off [y/N] "
else
    sig=TERM
    question="stop $name on $R_HOST? running tasks finish, or their leases return them to the queue [y/N] "
fi

# Runs on the box: $1 dir, $2 signal name.
STOP='cd "$1" || exit 3
[ -f logs/team.pid ] || { echo "not running (no pid file)" >&2; exit 4; }
pid=$(cat logs/team.pid)
kill -0 "$pid" 2>/dev/null || { echo "not running (pid $pid is gone)" >&2; exit 4; }
kill -s "$2" "$pid" || exit 5
: >logs/team.stopped
echo "$pid"'

cmd=$(remote_cmd "$STOP" "$R_DIR" "$sig")
if [ "$DRY" = 1 ]; then
    printf 'would ask: %s\n' "$question"
    run ssh -T $FT_SSH_OPTS "$R_HOST" "$cmd"
    exit 0
fi

if [ "$yes" = 0 ]; then
    printf '%s' "$question"
    ans=
    read -r ans || true
    case $ans in
        y | Y | yes | YES) ;;
        *) echo "not stopped"; exit 1 ;;
    esac
fi

set +e
pid=$(ssh -T $FT_SSH_OPTS "$R_HOST" "$cmd" </dev/null)
rc=$?
set -e
[ "$rc" = 0 ] || die "$name was not stopped (exit $rc)" 1
if [ "$sig" = TERM ]; then
    printf 'stopping %s · pid %s · draining (SIGTERM): running tasks finish or return to the queue\n' "$name" "$pid"
else
    printf 'stopped %s · pid %s (SIGKILL)\n' "$name" "$pid"
fi
