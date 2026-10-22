#!/bin/sh
# Move fantastic (every fantastic crate, one commit of branch main) to its
# newest commit, then build and test. Never commits by itself.
set -e
cd "$(dirname "$0")/.."
# Every fantastic package in the lock (all come from the one git commit).
set --
for n in $(awk -F'"' '/^name = "fantastic/{print $2}' Cargo.lock | sort -u); do
  set -- "$@" -p "$n"
done
cargo update "$@"
cargo build
cargo test
echo "fantastic now at:"
grep -A2 'name = "fantastic-kernel"' Cargo.lock | grep source
echo "commit with: git commit -am \"bump fantastic to <short sha>\""
