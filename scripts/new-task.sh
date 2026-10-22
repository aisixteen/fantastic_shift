#!/bin/sh
# new-task.sh — renders a task template (templates/<kind>.md) into a task file.
#   new-task.sh <kind> <title> [--set NAME=VALUE]… [--out FILE] [--force] [--print]
#   new-task.sh --list           the kinds and their fields
#   new-task.sh --check FILE     is the task ready? (no {{…}} and no WRITE: left)
# A template starts with a header comment that declares its fields:
#   <!-- template: KIND
#   field NAME            (required: give it with --set NAME=…)
#   field NAME=default    (optional)
#   -->
# TITLE, KIND, DATE and SLUG are filled by the generator; AUTHOR is $USER unless --set AUTHOR=….
set -eu
LC_ALL=C
export LC_ALL
PROG=new-task
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TPL="$ROOT/templates"
USAGE='usage: new-task.sh <kind> <title> [--set NAME=VALUE]… [--out FILE] [--force] [--print] | --list | --check FILE'

die() {
    printf '%s: %s\n' "$PROG" "$1" >&2
    exit "${2:-2}"
}

kinds() {
    for f in "$TPL"/*.md; do
        k=${f##*/}
        k=${k%.md}
        [ "$k" = README ] || printf ' %s' "$k"
    done
}

# fields FILE — one `NAME` or `NAME=default` per line, from the header comment.
fields() {
    awk 'NR == 1 { if ($0 !~ /^<!-- template: /) exit; next }
         /^-->$/ { exit }
         /^field / { sub(/^field /, ""); print }' "$1"
}

# check FILE — prints FILE:LINE: unfilled: TEXT for each {{ or WRITE: left; exit 1 if any.
check() {
    [ -f "$1" ] || die "no such file: $1"
    left=$(grep -n -e '{{' -e 'WRITE:' "$1" || true)
    if [ -n "$left" ]; then
        printf '%s\n' "$left" | while IFS= read -r l; do
            printf '%s:%s: unfilled: %s\n' "$1" "${l%%:*}" "${l#*:}"
        done
        exit 1
    fi
    printf '%s: ready\n' "$1"
    exit 0
}

[ $# -gt 0 ] || die "$USAGE"
case $1 in
    -h | --help) echo "$USAGE"; exit 0 ;;
    --list)
        for k in $(kinds); do
            printf '%s:' "$k"
            fields "$TPL/$k.md" | while IFS= read -r f; do printf ' %s' "$f"; done
            printf '\n'
        done
        exit 0 ;;
    --check) [ $# = 2 ] || die "$USAGE"; check "$2" ;;
esac

[ $# -ge 2 ] || die "$USAGE"
kind=$1 title=$2
shift 2
t="$TPL/$kind.md"
case $kind in '' | README | *[!a-z0-9-]*) t= ;; esac
[ -n "$t" ] && [ -f "$t" ] || die "no template \`$kind\` — kinds:$(kinds)"
[ -n "$(printf '%s' "$title" | tr -d ' ')" ] || die "the title is empty"

out= force=0 print=0 sets=
while [ $# -gt 0 ]; do
    case $1 in
        --set)
            [ $# -ge 2 ] || die "--set needs NAME=VALUE"
            case $2 in
                [A-Z]*=*) ;;
                *) die "--set takes NAME=VALUE, got \`$2\`" ;;
            esac
            n=${2%%=*}
            case $n in *[!A-Z0-9_]*) die "--set takes NAME=VALUE, got \`$2\`" ;; esac
            sets="$sets $n"
            export "NT_V_$n=${2#*=}"
            shift 2 ;;
        --out) [ $# -ge 2 ] || die "--out needs a file"; out=$2; shift 2 ;;
        --force) force=1; shift ;;
        --print) print=1; shift ;;
        *) die "unexpected argument \`$1\` — $USAGE" ;;
    esac
done

names=
missing=
for f in $(fields "$t" | sed 's/=.*//'); do names="$names $f"; done
for n in $sets; do
    case " $names AUTHOR " in
        *" $n "*) ;;
        *) die "$kind has no field \`$n\` — fields:$names" ;;
    esac
done
while IFS= read -r f; do
    n=${f%%=*}
    eval "given=\${NT_V_$n+x}"
    [ "$given" = x ] && continue
    case $f in
        *=*) export "NT_V_$n=${f#*=}" ;;
        *) missing="$missing --set $n=…" ;;
    esac
done <<EOF
$(fields "$t")
EOF
[ -z "$missing" ] || die "$kind needs$missing"

slug=$(printf '%s' "$title" | tr 'A-Z' 'a-z' | sed 's/[^a-z0-9][^a-z0-9]*/-/g; s/^-//; s/-$//' | cut -c1-60 | sed 's/-$//')
[ -n "$slug" ] || slug=task
export NT_V_TITLE="$title" NT_V_KIND="$kind" NT_V_SLUG="$slug"
export NT_V_DATE="$(date +%Y-%m-%d)"
[ "${NT_V_AUTHOR+x}" = x ] || export NT_V_AUTHOR="${USER:-someone}"

# Drop the header comment; replace {{NAME}} by $NT_V_NAME, literally.
body=$(awk '
    NR == 1 && /^<!-- template: / { head = 1; next }
    head { if ($0 == "-->") head = 0; next }
    {
        line = $0; res = ""
        while ((i = index(line, "{{")) > 0) {
            rest = substr(line, i + 2); j = index(rest, "}}")
            if (j == 0) break
            key = substr(rest, 1, j - 1)
            if (key ~ /^[A-Z][A-Z0-9_]*$/ && (("NT_V_" key) in ENVIRON)) {
                res = res substr(line, 1, i - 1) ENVIRON["NT_V_" key]; line = substr(rest, j + 2)
            } else {
                res = res substr(line, 1, i + 1); line = rest
            }
        }
        print res line
    }' "$t")
left=$(printf '%s\n' "$body" | grep -o '{{[^}]*}}' | sort -u | tr '\n' ' ' | sed 's/ *$//')
[ -z "$left" ] || die "template $kind leaves $left unfilled — declare them as fields" 3

if [ "$print" = 1 ]; then
    printf '%s\n' "$body"
    exit 0
fi
[ -n "$out" ] || out="tasks/$slug.md"
[ ! -e "$out" ] || [ "$force" = 1 ] || die "$out exists — --force to overwrite"
mkdir -p "$(dirname "$out")"
printf '%s\n' "$body" >"$out"
printf 'wrote %s (%s) — fill the WRITE: parts, then: scripts/new-task.sh --check %s\n' "$out" "$kind" "$out"
