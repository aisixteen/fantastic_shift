#!/bin/sh
# remote-ls.sh [--dry-run] — every remote in the laptop config and its state on the box.
set -eu
PROG=remote-ls
. "$(dirname "$0")/remote-common.sh"
USAGE='usage: remote-ls.sh [--dry-run]'

while [ $# -gt 0 ]; do
    case $1 in
        --dry-run) DRY=1; shift ;;
        -h | --help) echo "$USAGE"; exit 0 ;;
        *) die "unexpected argument \`$1\` — $USAGE" ;;
    esac
done

# Runs on the box: $1 dir. Prints one word, and the pid when there is one.
STATUS='cd "$1" 2>/dev/null || { echo nodir; exit 0; }
[ -f logs/team.pid ] || { echo never; exit 0; }
pid=$(cat logs/team.pid)
if kill -0 "$pid" 2>/dev/null; then echo "running $pid"; exit 0; fi
if [ -f logs/team.stopped ]; then echo "stopped $pid"; exit 0; fi
b=$(awk "/^btime/ {print \$2}" /proc/stat 2>/dev/null || true)
m=$(stat -c %Y logs/team.pid 2>/dev/null || true)
if [ -n "$b" ] && [ -n "$m" ] && [ "$m" -lt "$b" ]; then echo "rebooted $pid"; else echo "exited $pid"; fi'

names=
if [ -d "$FT_REMOTES" ]; then
    for f in "$FT_REMOTES"/*; do
        [ -f "$f" ] || continue
        n=${f##*/}
        valid_name "$n" || continue
        names="$names $n"
    done
fi
if [ -z "$names" ]; then
    echo "no remotes — add one: scripts/remote-add.sh <name> <host>:<dir>"
    exit 0
fi

[ "$DRY" = 1 ] || printf '%-12s %-28s %-6s %-9s %s\n' NAME HOST PORT ATTACHED STATE
for n in $names; do
    load_remote "$n"
    cmd=$(remote_cmd "$STATUS" "$R_DIR")
    if [ "$DRY" = 1 ]; then
        run ssh $FT_SSH_OPTS "$R_HOST" "$cmd"
        continue
    fi
    set +e
    out=$(ssh $FT_SSH_OPTS "$R_HOST" "$cmd" </dev/null 2>/dev/null)
    rc=$?
    set -e
    word=${out%% *} pid=${out#* }
    if [ "$rc" != 0 ]; then state=unreachable
    else
        case $word in
            running) state="running · pid $pid" ;;
            stopped) state="stopped" ;;
            rebooted) state="down — box restarted, run remote start" ;;
            exited) state="down — exited, see remote-logs" ;;
            never) state="not started" ;;
            nodir) state="no dir $R_DIR on the box" ;;
            *) state="unknown: $out" ;;
        esac
    fi
    attached=no
    a="$FT_RUN/$n.attach"
    if [ -f "$a" ] && kill -0 "$(cat "$a")" 2>/dev/null; then attached=yes; fi
    printf '%-12s %-28s %-6s %-9s %s\n' "$n" "$R_HOST" "${R_PORT:--}" "$attached" "$state"
done
