#!/usr/bin/env bash

set -eux

VERSION="${1:-'0.1.1-SNAPSHOT'}"

mkdir -p target/artifacts
cargo build --release --verbose --target aarch64-apple-darwin --config package.version=\"${VERSION}\"
cargo build --release --verbose --target x86_64-apple-darwin --config package.version=\"${VERSION}\"

lipo target/aarch64-apple-darwin/release/clu target/x86_64-apple-darwin/release/clu -create -output target/artifacts/clu

echo "Built a multi-arch binary attarget/artifacts/clu"
file target/artifacts/clu
target/artifacts/clu --help