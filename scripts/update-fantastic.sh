#!/bin/sh
# Move fantastic (all three crates) to the newest commit of its followed branch,
# then build and test. Never commits by itself.
set -e
cd "$(dirname "$0")/.."
cargo update -p fantastic-kernel -p fantastic-host -p fantastic-bundle -p fantastic-ai-core -p fantastic-credential-store -p fantastic-ram-store
cargo build
cargo test
echo "fantastic now at:"
grep -A2 'name = "fantastic-kernel"' Cargo.lock | grep source
echo "commit with: git commit -am \"bump fantastic to <short sha>\""
