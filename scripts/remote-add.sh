#!/bin/sh
# remote-add.sh <name> <host>:<dir> [--bin PATH] [--port N] [--keys "NAME …"] [--dry-run]
# Saves a remote in the laptop config (no secrets: --keys takes variable NAMES).
set -eu
PROG=remote-add
. "$(dirname "$0")/remote-common.sh"
USAGE='usage: remote-add.sh <name> <host>:<dir> [--bin PATH] [--port N] [--keys "NAME …"] [--dry-run]'

name= target= bin=fantastic_team port= keys=
while [ $# -gt 0 ]; do
    case $1 in
        --bin) [ $# -ge 2 ] || die "--bin needs a path"; bin=$2; shift 2 ;;
        --port) [ $# -ge 2 ] || die "--port needs a number"; port=$2; shift 2 ;;
        --keys) [ $# -ge 2 ] || die "--keys needs variable names"; keys=$2; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        -h | --help) echo "$USAGE"; exit 0 ;;
        -*) die "unknown flag \`$1\` — $USAGE" ;;
        *)
            if [ -z "$name" ]; then name=$1
            elif [ -z "$target" ]; then target=$1
            else die "unexpected argument \`$1\` — $USAGE"
            fi
            shift ;;
    esac
done
[ -n "$name" ] && [ -n "$target" ] || die "$USAGE"
valid_name "$name" || die "bad remote name \`$name\` — use letters, digits, - and _"
case $target in
    *:/*) host=${target%%:*} dir=${target#*:} ;;
    *) die "give the remote as <host>:<dir> with an absolute dir, got \`$target\`" ;;
esac
[ -n "$host" ] || die "give the remote as <host>:<dir> with an absolute dir, got \`$target\`"
[ -n "$bin" ] || die "--bin needs a path"
[ -z "$port" ] || valid_port "$port" || die "--port takes 1-65535, got \`$port\`"
for k in $keys; do
    valid_key "$k" || die "--keys takes variable names (letters, digits, _), got \`$k\` — never a value"
done

f="$FT_REMOTES/$name"
body="host=$host
dir=$dir
bin=$bin
port=$port
keys=$(echo $keys)"
if [ "$DRY" = 1 ]; then
    printf 'would write %s:\n%s\n' "$f" "$body"
    exit 0
fi
was=added
[ ! -f "$f" ] || was=updated
mkdir -p "$FT_REMOTES"
printf '%s\n' "$body" >"$f.tmp"
chmod 600 "$f.tmp"
mv "$f.tmp" "$f"
printf '%s %s · %s:%s · config %s\n' "$was" "$name" "$host" "$dir" "$f"
