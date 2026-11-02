#!/usr/bin/env sh

# Source this file before bootstrapping tools on networks in mainland China:
#   . scripts/use-domestic-mirrors.sh
#
# Cargo reads .cargo/config.toml directly. This script covers tool installation
# and browser/runtime downloads that do not use Cargo or npm package metadata.

export HPA_DOMESTIC_MIRRORS=1
export RUSTUP_DIST_SERVER="${RUSTUP_DIST_SERVER:-https://rsproxy.cn}"
export RUSTUP_UPDATE_ROOT="${RUSTUP_UPDATE_ROOT:-https://rsproxy.cn/rustup}"
export COREPACK_NPM_REGISTRY="${COREPACK_NPM_REGISTRY:-https://registry.npmmirror.com}"
export NODE_MIRROR="${NODE_MIRROR:-https://npmmirror.com/mirrors/node/}"
export NODEJS_ORG_MIRROR="${NODEJS_ORG_MIRROR:-https://npmmirror.com/mirrors/node/}"
export UV_DEFAULT_INDEX="${UV_DEFAULT_INDEX:-https://pypi.tuna.tsinghua.edu.cn/simple}"
export PIP_INDEX_URL="${PIP_INDEX_URL:-https://pypi.tuna.tsinghua.edu.cn/simple}"
export PLAYWRIGHT_DOWNLOAD_HOST="${PLAYWRIGHT_DOWNLOAD_HOST:-https://npmmirror.com/mirrors/playwright}"
export GOPROXY="${GOPROXY:-https://goproxy.cn,direct}"
export GOSUMDB="${GOSUMDB:-sum.golang.org}"

printf '%s\n' "Domestic download mirrors enabled:"
printf '%s\n' "  Cargo:      https://rsproxy.cn/index/"
printf '%s\n' "  npm/pnpm:   https://registry.npmmirror.com/"
printf '%s\n' "  Node.js:    https://npmmirror.com/mirrors/node/"
printf '%s\n' "  PyPI/uv:    https://pypi.tuna.tsinghua.edu.cn/simple"
printf '%s\n' "  Playwright: https://npmmirror.com/mirrors/playwright"
printf '%s\n' "  Rustup:     https://rsproxy.cn"
printf '%s\n' "  Go modules: https://goproxy.cn"
