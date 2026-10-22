#!/bin/sh
# sh tests/new_task.sh — tests for templates/*.md and scripts/new-task.sh.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
G="$ROOT/scripts/new-task.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/new_task.XXXXXX")
trap 'rm -rf "$T"' EXIT
PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "ok   $1"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL $1${2:+ — $2}"; }
has() { grep -qF -e "$2" "$1"; }
R="$T/out"
cd "$T" || exit 1


# 1. every_kind_is_listed
sh "$G" --list >"$R" 2>&1
n=0
for k in code-git code-github research docs triage; do grep -q "^$k:" "$R" || n=$((n + 1)); done
if [ "$n" = 0 ] && ! grep -q '^README' "$R"; then ok every_kind_is_listed
else bad every_kind_is_listed "$(cat "$R")"; fi

# 2. every_template_renders_with_no_unfilled_placeholder
for k in code-git code-github research docs triage; do
    flags=
    for f in $(awk 'NR>1 && /^-->$/ {exit} /^field / {sub(/^field /, ""); sub(/=.*/, ""); print}' "$ROOT/templates/$k.md"); do
        case $f in
            REPO) flags="$flags --set REPO=owner/name" ;;
            ISSUE) flags="$flags --set ISSUE=42" ;;
            BRANCH) flags="$flags --set BRANCH=fix/42-x" ;;
            CHECK) flags="$flags --set CHECK=make-test" ;;
            OUT) flags="$flags --set OUT=reports/x.md" ;;
            LABEL) flags="$flags --set LABEL=needs-triage" ;;
        esac
    done
    # shellcheck disable=SC2086
    sh "$G" "$k" "A $k task" $flags --print >"$R" 2>&1
    rc=$?
    if [ "$rc" = 0 ] && ! has "$R" '{{' && ! has "$R" '}}' && ! has "$R" '<!-- template:' \
        && has "$R" "# A $k task" && has "$R" "_Task kind: $k · written " \
        && has "$R" "## Rules — read first" && has "$R" "## The cycle (do all of it, in order)" \
        && has "$R" "## The task" && has "$R" "WRITE:"; then
        ok "renders_clean_$k"
    else bad "renders_clean_$k" "rc=$rc $(head -5 "$R")"; fi
done

# 3. fields_and_defaults_are_filled
sh "$G" code-github "Retry a dropped connection" --set REPO=owner/name --set ISSUE=42 \
    --set BRANCH=fix/42-retry --set "CHECK=cargo test -p x" --set AUTHOR=Ada --print >"$R" 2>&1
if has "$R" "# Retry a dropped connection" && has "$R" "written $(date +%Y-%m-%d) by Ada." \
    && has "$R" 'Change code in `owner/name` for issue #42' && has "$R" 'from `main`' \
    && has "$R" 'In WT run `cargo test -p x`.' && has "$R" 'Ask the `github` connector' \
    && has "$R" 'Ask the `git` connector' && has "$R" 'starts with `For #42.`'; then
    ok fields_and_defaults_are_filled
else bad fields_and_defaults_are_filled "$(head -20 "$R")"; fi

# 4. a_value_is_used_literally — &, \, / and | in a value are not special.
sh "$G" code-git "x" --set BRANCH=b --set 'CHECK=a && b | c \1 /d/' --print >"$R" 2>&1
if has "$R" 'In WT run `a && b | c \1 /d/`.'; then ok a_value_is_used_literally
else bad a_value_is_used_literally "$(grep 'In WT run' "$R")"; fi

# 5. a_default_can_be_overridden
sh "$G" docs "x" --set BRANCH=b --set BASE=develop --set "CHECK=mdlint ." --print >"$R" 2>&1
if has "$R" 'from `develop`' && has "$R" 'In WT run `mdlint .`.' && ! has "$R" 'git diff --check'; then
    ok a_default_can_be_overridden
else bad a_default_can_be_overridden "$(grep -e 'from' -e 'In WT' "$R")"; fi

# 6. a_missing_field_is_named
sh "$G" code-github "x" --set BRANCH=b >"$R" 2>&1
rc=$?
if [ "$rc" = 2 ] && [ "$(cat "$R")" = "new-task: code-github needs --set REPO=… --set ISSUE=… --set CHECK=…" ] && [ ! -d tasks ]; then
    ok a_missing_field_is_named
else bad a_missing_field_is_named "rc=$rc $(cat "$R")"; fi

# 7. an_unknown_field_is_refused
sh "$G" research "x" --set OUT=a.md --set FOO=1 >"$R" 2>&1
rc=$?
if [ "$rc" = 2 ] && [ "$(cat "$R")" = "new-task: research has no field \`FOO\` — fields: OUT" ]; then
    ok an_unknown_field_is_refused
else bad an_unknown_field_is_refused "rc=$rc $(cat "$R")"; fi

# 8. a_bad_set_is_refused
sh "$G" research "x" --set out=a.md >"$R" 2>&1
rc=$?
if [ "$rc" = 2 ] && [ "$(cat "$R")" = "new-task: --set takes NAME=VALUE, got \`out=a.md\`" ]; then
    ok a_bad_set_is_refused
else bad a_bad_set_is_refused "rc=$rc $(cat "$R")"; fi

# 9. an_unknown_kind_is_refused
sh "$G" poetry "x" >"$R" 2>&1
rc=$?
if [ "$rc" = 2 ] && [ "$(cat "$R")" = "new-task: no template \`poetry\` — kinds: code-git code-github docs research triage" ]; then
    ok an_unknown_kind_is_refused
else bad an_unknown_kind_is_refused "rc=$rc $(cat "$R")"; fi
sh "$G" README "x" >"$R" 2>&1
if [ $? = 2 ] && has "$R" "no template \`README\`"; then ok readme_is_not_a_kind
else bad readme_is_not_a_kind "$(cat "$R")"; fi

# 10. writes_the_task_file_by_slug
sh "$G" triage "Weekly triage: Oct/22!" --set REPO=o/n --set LABEL=new --set OUT=r.md >"$R" 2>&1
rc=$?
f=tasks/weekly-triage-oct-22.md
if [ "$rc" = 0 ] && [ -f "$f" ] \
    && [ "$(cat "$R")" = "wrote $f (triage) — fill the WRITE: parts, then: scripts/new-task.sh --check $f" ] \
    && has "$f" "# Weekly triage: Oct/22!" && ! has "$f" '{{'; then
    ok writes_the_task_file_by_slug
else bad writes_the_task_file_by_slug "rc=$rc $(cat "$R")"; fi

# 11. an_existing_file_is_kept_without_force
echo keep >"$T/mine.md"
sh "$G" research "x" --set OUT=a.md --out "$T/mine.md" >"$R" 2>&1
rc=$?
sh "$G" research "x" --set OUT=a.md --out "$T/mine.md" --force >"$R.2" 2>&1
rc2=$?
if [ "$rc" = 2 ] && has "$R" "new-task: $T/mine.md exists — --force to overwrite" \
    && [ "$rc2" = 0 ] && has "$T/mine.md" "_Task kind: research"; then
    ok an_existing_file_is_kept_without_force
else bad an_existing_file_is_kept_without_force "rc=$rc rc2=$rc2 $(cat "$R" "$R.2")"; fi

# 12. check_lists_what_is_unfilled
sh "$G" --check "$f" >"$R" 2>&1
rc=$?
if [ "$rc" = 1 ] && grep -q "^$f:[0-9][0-9]*: unfilled: .*WRITE:" "$R"; then
    ok check_lists_what_is_unfilled
else bad check_lists_what_is_unfilled "rc=$rc $(cat "$R")"; fi

# 13. check_passes_a_finished_task
grep -v 'WRITE:' "$f" >"$T/done.md"
sh "$G" --check "$T/done.md" >"$R" 2>&1
rc=$?
if [ "$rc" = 0 ] && [ "$(cat "$R")" = "$T/done.md: ready" ]; then
    ok check_passes_a_finished_task
else bad check_passes_a_finished_task "rc=$rc $(cat "$R")"; fi

# 14. a_template_with_an_undeclared_placeholder_is_refused — on a copy of the repo.
mkdir -p "$T/copy/scripts" "$T/copy/templates"
cp "$G" "$T/copy/scripts/new-task.sh"
printf '<!-- template: bad\nfield A\n-->\n# {{TITLE}} {{A}} {{B}}\n' >"$T/copy/templates/bad.md"
sh "$T/copy/scripts/new-task.sh" bad "x" --set A=1 --print >"$R" 2>&1
rc=$?
if [ "$rc" = 3 ] && has "$R" "new-task: template bad leaves {{B}} unfilled — declare them as fields"; then
    ok a_template_with_an_undeclared_placeholder_is_refused
else bad a_template_with_an_undeclared_placeholder_is_refused "rc=$rc $(cat "$R")"; fi

# 15. no_template_holds_a_secret_like_string
if grep -nE -e 'sk-[A-Za-z0-9]{8}' -e 'gh[pousr]_[A-Za-z0-9]{8}' -e 'BEGIN [A-Z ]*PRIVATE KEY' "$ROOT"/templates/*.md; then
    bad no_template_holds_a_secret_like_string
else ok no_template_holds_a_secret_like_string; fi

# 16. no_template_closes_an_issue
if grep -niE '(close[sd]?|fix(e[sd])?|resolve[sd]?) #[0-9{]' "$ROOT"/templates/*.md; then
    bad no_template_closes_an_issue
else ok no_template_closes_an_issue; fi

# 17. the_generator_parses_as_posix_sh
n=0
sh -n "$G" || n=1
if command -v dash >/dev/null 2>&1; then dash -n "$G" || n=1; fi
if [ "$n" = 0 ]; then ok the_generator_parses_as_posix_sh; else bad the_generator_parses_as_posix_sh; fi

echo "new_task: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
