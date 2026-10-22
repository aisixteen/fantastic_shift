#!/bin/sh
# sh tests/remote_scripts.sh — tests for scripts/remote-*.sh. No network, no real
# ssh: a stub `ssh` first on PATH records its argv and stdin; with STUB_BOX=1 it
# also runs the remote command locally, in a clean environment, as the box would.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
S="$ROOT/scripts"
T=$(mktemp -d "${TMPDIR:-/tmp}/remote_scripts.XXXXXX")
PASS=0
FAIL=0
BOXPID=

cleanup() {
    [ -z "$BOXPID" ] || kill -9 "$BOXPID" 2>/dev/null
    rm -rf "$T"
}
trap cleanup EXIT

ok() { PASS=$((PASS + 1)); echo "ok   $1"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL $1${2:+ — $2}"; }
has() { grep -qF -e "$2" "$1"; }      # has FILE TEXT
calls() { [ -f "$T/log/argv" ] && grep -c '^--- call$' "$T/log/argv" || echo 0; }

# The stub ssh.
mkdir -p "$T/bin" "$T/log" "$T/box" "$T/fake" "$T/cfg"
cat >"$T/bin/ssh" <<'STUB'
#!/bin/sh
{ echo "--- call"; for a in "$@"; do printf '%s\n' "$a"; done; } >>"$STUB_LOG/argv"
tunnel=0 last=
for a in "$@"; do [ "$a" = -N ] && tunnel=1; last=$a; done
if [ "$tunnel" = 1 ]; then exit "${STUB_RC:-255}"; fi
cat >"$STUB_LOG/stdin"
if [ "${STUB_BOX:-0}" = 1 ]; then
    exec env -i PATH="$PATH" HOME="$HOME" FAKE_OUT="$FAKE_OUT" sh -c "$last" <"$STUB_LOG/stdin"
fi
exit "${STUB_RC:-0}"
STUB
chmod +x "$T/bin/ssh"

# The fake binary on the box: records its argv and a checksum of the key it got.
cat >"$T/box/fakebin" <<'FAKE'
#!/bin/sh
printf '%s\n' "$@" >"$FAKE_OUT/args"
printf '%s' "${SHIFT_TEST_KEY:-}" | cksum >"$FAKE_OUT/key.cksum"
trap 'echo term >"$FAKE_OUT/signal"; exit 0' TERM
echo "fake team up"
while :; do sleep 1; done
FAKE
chmod +x "$T/box/fakebin"

PATH="$T/bin:$PATH"
XDG_CONFIG_HOME="$T/cfg"
STUB_LOG="$T/log"
FAKE_OUT="$T/fake"
export PATH XDG_CONFIG_HOME STUB_LOG FAKE_OUT
unset STUB_BOX STUB_RC FT_ATTACH_TRIES FT_ATTACH_DELAY

# A fake key, built from pieces (with a space and a quote, to test the quoting).
KEY="ft-$(printf 'sec')ret $(date +%s)'$$-$(printf 'Zq')"
KEY64=$(printf '%s' "$KEY" | base64 | tr -d '\n')
CFG="$T/cfg/fantastic-team/remotes"
R="$T/out"

# 1. add_writes_config_without_secrets
sh "$S/remote-add.sh" gpu "box.example:$T/box" --bin "$T/box/fakebin" --port 8093 --keys "SHIFT_TEST_KEY" >"$R" 2>&1
rc=$?
exp="host=box.example
dir=$T/box
bin=$T/box/fakebin
port=8093
keys=SHIFT_TEST_KEY"
if [ "$rc" = 0 ] && has "$R" "added gpu · box.example:$T/box · config $CFG/gpu" \
    && [ "$(cat "$CFG/gpu")" = "$exp" ] && ! has "$CFG/gpu" "$KEY"; then
    ok add_writes_config_without_secrets
else bad add_writes_config_without_secrets "rc=$rc $(cat "$R")"; fi

# 2. add_refuses_bad_input
n=0
for args in "g/p box:/srv" "gpu2 box" "gpu2 box:relative" "gpu2 box:/srv --port 70000"; do
    # shellcheck disable=SC2086
    sh "$S/remote-add.sh" $args >"$R" 2>&1 && n=$((n + 1))
done
sh "$S/remote-add.sh" gpu2 box:/srv --keys "A=B" >"$R" 2>&1 && n=$((n + 1))
if [ "$n" = 0 ] && has "$R" "remote-add: --keys takes variable names (letters, digits, _), got \`A=B\` — never a value" \
    && [ ! -f "$CFG/gpu2" ]; then
    ok add_refuses_bad_input
else bad add_refuses_bad_input "$n accepted; $(cat "$R")"; fi

# 3. add_dry_run_writes_nothing
sh "$S/remote-add.sh" gpu3 box:/srv --dry-run >"$R" 2>&1
if [ $? = 0 ] && has "$R" "would write $CFG/gpu3:" && has "$R" "host=box" && [ ! -f "$CFG/gpu3" ]; then
    ok add_dry_run_writes_nothing
else bad add_dry_run_writes_nothing "$(cat "$R")"; fi

# 4. start_refuses_missing_key
unset SHIFT_TEST_KEY
sh "$S/remote-start.sh" gpu >"$R" 2>&1
rc=$?
if [ "$rc" = 2 ] && [ "$(cat "$R")" = "remote-start: SHIFT_TEST_KEY is not set on this laptop — export it, then run again" ] \
    && [ "$(calls)" = 0 ]; then
    ok start_refuses_missing_key
else bad start_refuses_missing_key "rc=$rc $(cat "$R")"; fi

SHIFT_TEST_KEY=$KEY
export SHIFT_TEST_KEY

# 5. start_dry_run_has_no_secret
sh "$S/remote-start.sh" gpu --dry-run >"$R" 2>&1
rc=$?
if [ "$rc" = 0 ] && has "$R" "+ ssh -T -o BatchMode=yes" && has "$R" "box.example" \
    && has "$R" "--host 127.0.0.1 --port" && has "$R" "nohup" \
    && has "$R" "stdin: SHIFT_TEST_KEY=<base64, hidden>" \
    && ! has "$R" "$KEY" && ! has "$R" "$KEY64" && ! has "$R" "0.0.0.0" && [ "$(calls)" = 0 ]; then
    ok start_dry_run_has_no_secret
else bad start_dry_run_has_no_secret "rc=$rc $(cat "$R")"; fi

# 6. start_pipes_keys_on_stdin_not_argv — runs the remote command locally (STUB_BOX=1).
STUB_BOX=1 sh "$S/remote-start.sh" gpu >"$R" 2>&1 &
sp=$!
i=0
while kill -0 "$sp" 2>/dev/null && [ "$i" -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
if kill -0 "$sp" 2>/dev/null; then
    kill "$sp"
    bad start_pipes_keys_on_stdin_not_argv "did not return within 10 s"
else
    wait "$sp"
    rc=$?
    BOXPID=$(sed -n 's/^started gpu · pid \([0-9][0-9]*\) · log .*$/\1/p' "$R")
    i=0
    while [ ! -f "$T/fake/key.cksum" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
    want_args="--dir
$T/box
--host
127.0.0.1
--port
8093"
    why=
    [ "$rc" = 0 ] || why="$why rc=$rc"
    has "$R" "started gpu · pid $BOXPID · log $T/box/logs/kernel.log" || why="$why no-started-line"
    [ -n "$BOXPID" ] && kill -0 "$BOXPID" 2>/dev/null || why="$why not-running"
    ! has "$T/log/argv" "$KEY" && ! has "$T/log/argv" "$KEY64" || why="$why key-in-ssh-argv"
    has "$T/log/stdin" "SHIFT_TEST_KEY=$KEY64" || why="$why key-not-on-stdin"
    [ "$(cat "$T/fake/key.cksum" 2>/dev/null)" = "$(printf '%s' "$KEY" | cksum)" ] || why="$why box-got-wrong-key"
    [ "$(cat "$T/fake/args" 2>/dev/null)" = "$want_args" ] || why="$why args=$(cat "$T/fake/args" 2>/dev/null)"
    ps -o args= -p "$BOXPID" 2>/dev/null | grep -qF -e "$KEY" && why="$why key-in-ps"
    grep -rqF -e "$KEY" "$T/cfg" "$T/box" && why="$why key-in-a-file"
    grep -rqF -e "$KEY64" "$T/cfg" "$T/box" && why="$why key64-in-a-file"
    if [ -z "$why" ]; then ok start_pipes_keys_on_stdin_not_argv
    else bad start_pipes_keys_on_stdin_not_argv "$why; $(cat "$R")"; fi
fi

# 7. start_refuses_a_second_start
STUB_BOX=1 sh "$S/remote-start.sh" gpu >"$R" 2>&1
rc=$?
if [ "$rc" = 1 ] && has "$R" "already running · pid $BOXPID" && has "$R" "remote-start: gpu did not start on box.example (exit 4)"; then
    ok start_refuses_a_second_start
else bad start_refuses_a_second_start "rc=$rc $(cat "$R")"; fi

# 8. ls_shows_running
STUB_BOX=1 sh "$S/remote-ls.sh" >"$R" 2>&1
rc=$?
if [ "$rc" = 0 ] && has "$R" "NAME" && has "$R" "ATTACHED" \
    && grep -q "^gpu .*box.example .*8093 .*no .*running · pid $BOXPID\$" "$R"; then
    ok ls_shows_running
else bad ls_shows_running "rc=$rc $(cat "$R")"; fi

# 9. attach_dry_run_prints_the_tunnel
c0=$(calls)
sh "$S/remote-attach.sh" gpu --dry-run >"$R" 2>&1
rc=$?
if [ "$rc" = 0 ] && has "$R" "+ ssh -N" && has "$R" "-L 127.0.0.1:8093:127.0.0.1:8093 box.example" \
    && has "$R" "ServerAliveInterval=15" && has "$R" "ExitOnForwardFailure=yes" \
    && has "$R" "url: http://127.0.0.1:8093" && [ "$(calls)" = "$c0" ]; then
    ok attach_dry_run_prints_the_tunnel
else bad attach_dry_run_prints_the_tunnel "rc=$rc $(cat "$R")"; fi

# 10. attach_reopens_a_dropped_tunnel — the stub tunnel drops at once, three times.
c0=$(calls)
FT_ATTACH_TRIES=3 FT_ATTACH_DELAY=0 STUB_RC=255 sh "$S/remote-attach.sh" gpu >"$R" 2>&1
rc=$?
c1=$(calls)
if [ "$rc" = 1 ] && [ $((c1 - c0)) = 3 ] && has "$R" "attached to gpu at http://127.0.0.1:8093 — Ctrl-C detaches; gpu keeps running" \
    && [ "$(grep -c 'tunnel to gpu dropped — reopening in 0 s' "$R")" = 2 ] \
    && has "$R" "remote-attach: the tunnel to gpu dropped 3 times — giving up; gpu keeps running" \
    && kill -0 "$BOXPID" 2>/dev/null && [ ! -f "$T/cfg/fantastic-team/run/gpu.attach" ]; then
    ok attach_reopens_a_dropped_tunnel
else bad attach_reopens_a_dropped_tunnel "rc=$rc calls=$((c1 - c0)) $(cat "$R")"; fi

# 11. attach_tui_is_refused_for_now
sh "$S/remote-attach.sh" gpu --tui >"$R" 2>&1
rc=$?
if [ "$rc" = 2 ] && has "$R" "remote-attach: --tui needs the binary's terminal UI over the tunnel (#38)"; then
    ok attach_tui_is_refused_for_now
else bad attach_tui_is_refused_for_now "rc=$rc $(cat "$R")"; fi

# 12. logs_tails_the_box_log
STUB_BOX=1 sh "$S/remote-logs.sh" gpu -n 5 >"$R" 2>&1
rc=$?
if [ "$rc" = 0 ] && has "$R" "fake team up"; then
    ok logs_tails_the_box_log
else bad logs_tails_the_box_log "rc=$rc $(cat "$R")"; fi
sh "$S/remote-logs.sh" gpu -f --dry-run >"$R" 2>&1
if has "$R" "+ ssh -o BatchMode=yes" && has "$R" "tail -n" && ! has "$R" "$KEY"; then
    ok logs_dry_run
else bad logs_dry_run "$(cat "$R")"; fi

# 13. stop_dry_run_asks_and_names_the_signal
c0=$(calls)
sh "$S/remote-stop.sh" gpu --dry-run >"$R" 2>&1
sh "$S/remote-stop.sh" gpu --now --dry-run >"$R.now" 2>&1
if has "$R" "would ask: stop gpu on box.example? running tasks finish, or their leases return them to the queue [y/N]" \
    && has "$R" "'TERM'" && has "$R.now" "would ask: stop gpu on box.example NOW? running tasks are cut off [y/N]" \
    && has "$R.now" "'KILL'" && [ "$(calls)" = "$c0" ]; then
    ok stop_dry_run_asks_and_names_the_signal
else bad stop_dry_run_asks_and_names_the_signal "$(cat "$R" "$R.now")"; fi

# 14. stop_asks_first — answering n stops nothing and calls no ssh.
c0=$(calls)
echo n | STUB_BOX=1 sh "$S/remote-stop.sh" gpu >"$R" 2>&1
rc=$?
if [ "$rc" = 1 ] && has "$R" "stop gpu on box.example? running tasks finish, or their leases return them to the queue [y/N] not stopped" \
    && [ "$(calls)" = "$c0" ] && kill -0 "$BOXPID" 2>/dev/null; then
    ok stop_asks_first
else bad stop_asks_first "rc=$rc $(cat "$R")"; fi

# 15. stop_drains_with_sigterm — answering y sends TERM; ls then says stopped.
echo y | STUB_BOX=1 sh "$S/remote-stop.sh" gpu >"$R" 2>&1
rc=$?
i=0
while kill -0 "$BOXPID" 2>/dev/null && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
STUB_BOX=1 sh "$S/remote-ls.sh" >"$R.ls" 2>&1
if [ "$rc" = 0 ] && has "$R" "stopping gpu · pid $BOXPID · draining (SIGTERM): running tasks finish or return to the queue" \
    && [ "$(cat "$T/fake/signal" 2>/dev/null)" = term ] && ! kill -0 "$BOXPID" 2>/dev/null \
    && grep -q "^gpu .* stopped\$" "$R.ls"; then
    ok stop_drains_with_sigterm
else bad stop_drains_with_sigterm "rc=$rc $(cat "$R" "$R.ls")"; fi

# 16. no_file_holds_the_key — the config, the box dir and its log.
if grep -rqF -e "$KEY" "$T/cfg" "$T/box" "$T/fake" || grep -rqF -e "$KEY64" "$T/cfg" "$T/box" "$T/fake"; then
    bad no_file_holds_the_key
else ok no_file_holds_the_key; fi

# 17. scripts_parse_as_posix_sh
n=0
for f in "$S"/remote-*.sh; do
    sh -n "$f" || n=$((n + 1))
    if command -v dash >/dev/null 2>&1; then dash -n "$f" || n=$((n + 1)); fi
done
if [ "$n" = 0 ]; then ok scripts_parse_as_posix_sh; else bad scripts_parse_as_posix_sh "$n failed"; fi

echo "remote_scripts: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
