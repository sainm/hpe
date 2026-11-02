#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tool_dir="${HPA_TOOL_DIR:-$root_dir/target/tools}"
version="${COSIGN_VERSION:-v2.6.0}"

mkdir -p "$tool_dir"
GOPROXY="${GOPROXY:-https://goproxy.cn,direct}" \
GOSUMDB="${GOSUMDB:-sum.golang.org}" \
GOBIN="$tool_dir" \
  go install "github.com/sigstore/cosign/v2/cmd/cosign@$version"

"$tool_dir/cosign" version
shasum -a 256 "$tool_dir/cosign"
