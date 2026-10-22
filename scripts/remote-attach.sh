#!/bin/sh
# remote-attach.sh <name> [--port N] [--tui] [--dry-run]
# Opens an SSH tunnel 127.0.0.1:N (laptop) → 127.0.0.1:N (box) with keep-alives,
# re-opens it when it drops, and prints the local URL. Ctrl-C detaches: it closes
# only the tunnel; the shift on the box keeps running. Needs no keys.
set -eu
PROG=remote-attach
. "$(dirname "$0")/remote-common.sh"
USAGE='usage: remote-attach.sh <name> [--port N] [--tui] [--dry-run]'

name= port= tui=0
while [ $# -gt 0 ]; do
    case $1 in
        --port) [ $# -ge 2 ] || die "--port needs a number"; port=$2; shift 2 ;;
        --tui) tui=1; shift ;;
        --dry-run) DRY=1; shift ;;
        -h | --help) echo "$USAGE"; exit 0 ;;
        -*) die "unknown flag \`$1\` — $USAGE" ;;
        *) [ -z "$name" ] || die "unexpected argument \`$1\` — $USAGE"; name=$1; shift ;;
    esac
done
[ -n "$name" ] || die "$USAGE"
load_remote "$name"
[ "$tui" = 0 ] || die "--tui needs the binary's terminal UI over the tunnel (#38), not there yet — use the web UI: remote-attach.sh $name"
[ -n "$port" ] || port=$R_PORT
[ -n "$port" ] || die "$name has no port — start it with one (remote-start.sh $name --port 8093) and attach with --port 8093, or save it: remote-add.sh $name $R_HOST:$R_DIR --port 8093"
valid_port "$port" || die "--port takes 1-65535, got \`$port\`"

url="http://127.0.0.1:$port"
if [ "$DRY" = 1 ]; then
    run ssh -N $FT_SSH_OPTS -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -L "127.0.0.1:$port:127.0.0.1:$port" "$R_HOST"
    printf 'url: %s\n' "$url"
    exit 0
fi

mkdir -p "$FT_RUN"
mark="$FT_RUN/$name.attach"
echo $$ >"$mark"
trap 'rm -f "$mark"; printf "\ndetached from %s; %s keeps running\n" "$name" "$name"; exit 0' INT TERM
printf 'attached to %s at %s — Ctrl-C detaches; %s keeps running\n' "$name" "$url" "$name"
tries=0
delay=${FT_ATTACH_DELAY:-2}
while :; do
    ssh -N $FT_SSH_OPTS -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -L "127.0.0.1:$port:127.0.0.1:$port" "$R_HOST" </dev/null || true
    tries=$((tries + 1))
    if [ -n "${FT_ATTACH_TRIES:-}" ] && [ "$tries" -ge "$FT_ATTACH_TRIES" ]; then
        rm -f "$mark"
        die "the tunnel to $name dropped $tries times — giving up; $name keeps running" 1
    fi
    printf 'tunnel to %s dropped — reopening in %s s\n' "$name" "$delay" >&2
    sleep "$delay"
done
