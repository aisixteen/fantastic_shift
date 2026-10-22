#!/bin/sh
# remote-start.sh <name> [--port N] [--dry-run]
# Starts the shift on the box DETACHED and returns at once. The values of the
# variables named by the remote's keys= are read here, on the laptop, and piped
# over SSH stdin as NAME=<base64> lines into the remote shell's memory: never in
# an argv (ps), never in a file, never in shell history. The binary binds
# 127.0.0.1 only; reach it with remote-attach.sh.
set -eu
PROG=remote-start
. "$(dirname "$0")/remote-common.sh"
USAGE='usage: remote-start.sh <name> [--port N] [--dry-run]'

name= port=
while [ $# -gt 0 ]; do
    case $1 in
        --port) [ $# -ge 2 ] || die "--port needs a number"; port=$2; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        -h | --help) echo "$USAGE"; exit 0 ;;
        -*) die "unknown flag \`$1\` — $USAGE" ;;
        *) [ -z "$name" ] || die "unexpected argument \`$1\` — $USAGE"; name=$1; shift ;;
    esac
done
[ -n "$name" ] || die "$USAGE"
load_remote "$name"
[ -n "$port" ] || port=$R_PORT
[ -z "$port" ] || valid_port "$port" || die "--port takes 1-65535, got \`$port\`"
for k in $R_KEYS; do
    eval "isset=\${$k+x}"
    [ "$isset" = x ] || die "$k is not set on this laptop — export it, then run again"
done

# Runs on the box: $1 dir, $2 binary, $3 port (may be empty). Key lines on stdin.
START='d=$1 b=$2 p=$3
cd "$d" || { echo "no such dir on the box: $d" >&2; exit 3; }
mkdir -p logs
if [ -f logs/shift.pid ] && kill -0 "$(cat logs/shift.pid)" 2>/dev/null; then
  echo "already running · pid $(cat logs/shift.pid)" >&2; exit 4
fi
while IFS= read -r l; do
  n=${l%%=*}
  case $n in ""|[0-9]*|*[!A-Za-z0-9_]*) echo "bad key line on stdin" >&2; exit 5;; esac
  v=$(printf "%s" "${l#*=}" | base64 -d) || { echo "bad base64 for $n" >&2; exit 5; }
  export "$n=$v"
done
v=
set -- --dir "$d"
if [ -n "$p" ]; then set -- "$@" --host 127.0.0.1 --port "$p"; fi
rm -f logs/shift.stopped
nohup "$b" "$@" </dev/null >>logs/kernel.log 2>&1 &
echo $! >logs/shift.pid
echo "$! $d/logs/kernel.log"'

cmd=$(remote_cmd "$START" "$R_DIR" "$R_BIN" "$port")

# NAME=<base64 of the value> per key, on stdout (a pipe to ssh). printf is a
# shell builtin, so a value never becomes an argv.
key_lines() {
    for k in $R_KEYS; do
        eval "v=\${$k}"
        printf '%s=' "$k"
        printf '%s' "$v" | base64 | tr -d '\n'
        printf '\n'
    done
    v=
}

if [ "$DRY" = 1 ]; then
    run ssh -T $FT_SSH_OPTS "$R_HOST" "$cmd"
    for k in $R_KEYS; do printf 'stdin: %s=<base64, hidden>\n' "$k"; done
    exit 0
fi

set +e
out=$(key_lines | ssh -T $FT_SSH_OPTS "$R_HOST" "$cmd")
rc=$?
set -e
[ "$rc" = 0 ] || die "$name did not start on $R_HOST (exit $rc)" 1
pid=${out%% *} log=${out#* }
printf 'started %s · pid %s · log %s\n' "$name" "$pid" "$log"
if [ -n "$port" ]; then
    printf 'attach: scripts/remote-attach.sh %s --port %s\n' "$name" "$port"
fi
