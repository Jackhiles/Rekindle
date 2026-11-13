#!/bin/sh
# Build Rekindle and put the "Rekindle" program in this folder (run it to open the launcher).
set -e
cd "$(dirname "$0")"
cargo build --release
# Replacing the file is safe even while it runs (the old copy stays open until it exits).
cp target/release/rekindle Rekindle.new
chmod +x Rekindle.new
mv -f Rekindle.new Rekindle
echo "Built ./Rekindle - run it to open the launcher."
