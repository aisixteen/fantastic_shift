#!/bin/sh
# Move fantastic (every fantastic crate, one commit of branch main) to its
# newest commit, then build and test. Never commits by itself.
set -e
cd "$(dirname "$0")/.."
cargo update -p fantastic-tui -p fantastic-kernel -p fantastic-bundle
cargo build
cargo test
echo "fantastic now at:"
grep -A2 'name = "fantastic-kernel"' Cargo.lock | grep source
echo "commit with: git commit -am \"bump fantastic to <short sha>\""
