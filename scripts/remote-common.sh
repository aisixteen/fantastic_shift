# remote-common.sh — sourced by scripts/remote-*.sh (POSIX sh). Not run on its own.
# The laptop config of a remote is one file of key=value lines under
# ${XDG_CONFIG_HOME:-$HOME/.config}/fantastic-team/remotes/<name>. It holds no secret:
# `keys=` lists environment variable NAMES; remote-start.sh reads their values
# on the laptop and pipes them to the box over SSH stdin.

FT_CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/fantastic-team"
FT_REMOTES="$FT_CONFIG_DIR/remotes"
FT_RUN="$FT_CONFIG_DIR/run"
FT_SSH_OPTS="-o BatchMode=yes -o ConnectTimeout=10"
DRY=0
LC_ALL=C
export LC_ALL

die() {
    printf '%s: %s\n' "$PROG" "$1" >&2
    exit "${2:-2}"
}

# q WORD — WORD single-quoted for a POSIX shell.
q() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# show WORD — WORD as is when it is plain, else single-quoted.
show() {
    case $1 in
        '' | *[!A-Za-z0-9_./:=@,+-]*) q "$1" ;;
        *) printf '%s' "$1" ;;
    esac
}

# run CMD ARGS… — runs it, or with --dry-run prints it as `+ CMD ARGS…`.
run() {
    if [ "$DRY" = 1 ]; then
        printf '+'
        for a in "$@"; do printf ' %s' "$(show "$a")"; done
        printf '\n'
        return 0
    fi
    "$@"
}

valid_name() {
    case $1 in '' | *[!A-Za-z0-9_-]*) return 1 ;; esac
    return 0
}

valid_key() {
    case $1 in '' | [0-9]* | *[!A-Za-z0-9_]*) return 1 ;; esac
    return 0
}

valid_port() {
    case $1 in '' | *[!0-9]*) return 1 ;; esac
    [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}

# remote_cmd SCRIPT ARGS… — one string for ssh: `sh -c 'SCRIPT' ft ARGS…`, all quoted.
remote_cmd() {
    s=$1
    shift
    c="sh -c $(q "$s") ft"
    for a in "$@"; do c="$c $(q "$a")"; done
    printf '%s' "$c"
}

# load_remote NAME — sets R_NAME R_HOST R_DIR R_BIN R_PORT R_KEYS from the config file.
load_remote() {
    valid_name "$1" || die "bad remote name \`$1\` — use letters, digits, - and _"
    f="$FT_REMOTES/$1"
    [ -f "$f" ] || die "no remote \`$1\` — add it: scripts/remote-add.sh $1 <host>:<dir>"
    R_NAME=$1 R_HOST= R_DIR= R_BIN=fantastic_team R_PORT= R_KEYS=
    while IFS= read -r line || [ -n "$line" ]; do
        case $line in
            host=*) R_HOST=${line#host=} ;;
            dir=*) R_DIR=${line#dir=} ;;
            bin=*) R_BIN=${line#bin=} ;;
            port=*) R_PORT=${line#port=} ;;
            keys=*) R_KEYS=${line#keys=} ;;
        esac
    done <"$f"
    [ -n "$R_HOST" ] && [ -n "$R_DIR" ] || die "$f needs a host= and a dir= line"
    for k in $R_KEYS; do
        valid_key "$k" || die "$f: keys= takes variable names, got \`$k\`"
    done
}
