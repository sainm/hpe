# 国内依赖下载源

> 适用范围：中国大陆网络中的 Rust、Node/pnpm、Python/uv、Playwright、Go 与 Rustup 引导。镜像只改变传输来源，不改变锁文件、包名、版本、SHA-256 或签名校验。

## 1. 固定镜像

| 生态 | 下载源 | 仓库位置 | 完整性 |
| --- | --- | --- | --- |
| Rust crates | `https://rsproxy.cn/index/` | `.cargo/config.toml` | `Cargo.lock` checksum |
| npm / pnpm | `https://registry.npmmirror.com/` | `.npmrc` | `pnpm-lock.yaml` integrity |
| Node.js 26.13 | `https://npmmirror.com/mirrors/node/` | `scripts/use-domestic-mirrors.sh`、CI | Node release checksum |
| Python / uv | `https://pypi.tuna.tsinghua.edu.cn/simple` | `services/ai-worker/uv.lock` | artifact SHA-256 |
| Playwright | `https://npmmirror.com/mirrors/playwright` | `scripts/use-domestic-mirrors.sh` | Playwright 自带 checksum |
| Rustup | `https://rsproxy.cn` | `scripts/use-domestic-mirrors.sh` | Rust release checksum |
| AI Worker 基础镜像 | `docker.m.daocloud.io/library/python:3.13-slim` | `services/ai-worker/Containerfile` | 固定 manifest digest |
| Go modules / cosign | `https://goproxy.cn,direct` | `scripts/install-cosign-domestic.sh` | Go module version + `sum.golang.org` |
| PostgreSQL CI 镜像 | `docker.m.daocloud.io/library/postgres:18` | `.github/workflows/ci.yml` | 固定 manifest digest |
| S3 Connector CI 镜像 | `docker.m.daocloud.io/bitnamilegacy/minio` | `.github/workflows/ci.yml` | 固定 manifest digest |
| OTel/Grafana demo 镜像 | `docker.m.daocloud.io/otel|grafana|prom/...` | `deploy/otel/compose.yaml` | 固定 manifest digest；生产仍须镜像签名验证 |

初始化前执行：

```bash
. scripts/use-domestic-mirrors.sh
node --version
pnpm install --frozen-lockfile
(cd services/ai-worker && uv sync --frozen)
pnpm check:mirrors
```

Cargo 不需要额外环境变量；在仓库目录内会通过 `.cargo/config.toml` 使用稀疏镜像索引。Python 锁文件已经固定清华 PyPI artifact URL，因此 CI 和本地安装都不应在无审查的情况下重新写入锁文件。

Worker 容器基础镜像通过 DaoCloud 公共加速域名拉取，但 `Containerfile` 固定 manifest digest；仅固定可读 tag 不能作为发布身份。

cosign 从国内 Go proxy 编译固定版本到 `target/tools`：

```bash
./scripts/install-cosign-domestic.sh
```

脚本保持 `GOSUMDB=sum.golang.org`，记录二进制 SHA-256，但不替代 Sigstore bundle 的证书、身份、Rekor 或公钥验证。

未预装目标版本时，可用国内 npm 源一次性启动声明的 Node/pnpm：

```bash
npm_config_registry=https://registry.npmmirror.com \
  npx -y -p node@26.13.0 -p pnpm@12.16.1 pnpm check
```

## 2. 安全边界

- 不关闭 `pnpm install --frozen-lockfile`、`uv sync --frozen` 或 Cargo checksum 校验。
- 不使用 `npm install` 替代 pnpm，不提交新的 `package-lock.json`。
- npm audit、`cargo-deny advisories` 与 `pip-audit` 仍查询权威安全公告；镜像下载成功不等于漏洞检查通过。当前 `registry.npmmirror.com` 没有实现 `/-/npm/v1/security/advisories/bulk`，因此 npm audit 保留 `registry.npmjs.org` 作为安全元数据例外；不得把包下载 registry 与该端点混为一谈。
- 镜像不可达时构建应失败，不自动回退到未配置的任意来源。
- 发布 SBOM、签名和 checksums 时记录最终 artifact 摘要，不记录临时下载 URL 作为供应链身份。
- Docker 镜像仓库配置属于主机 daemon 或集群策略，不在 Git 仓库中写入公共加速地址；生产镜像必须固定 digest 并经过镜像签名验证。
- RustSec advisory database 当前没有已验证的国内公共镜像；`cargo-deny` 的实时公告更新仍要求访问 GitHub 或企业自建可信镜像。隔离网络可预同步公告库后执行 `cargo-deny --offline check ...`，并记录同步时间和 commit；不得把“镜像下载成功”当作“漏洞库已更新”。
- GitHub Actions 自身的 runner、Action checkout 与云端服务不属于项目依赖下载；若企业要求完全境内构建，应把同一命令迁入企业 Runner 和内网 Artifactory/Nexus，不能只改包 registry 就宣称构建链路已完全国产化。
- `pnpm check:mirrors` 静态校验 Cargo/npm/Python/Playwright/Go 配置、Python 锁文件、Containerfile、Compose 和 CI service 镜像；容器镜像必须同时使用 DaoCloud 域名与固定 SHA-256 digest。

## 3. 企业网络切换

若企业必须使用内部 Artifactory/Nexus：

1. 同步 Rust sparse index、npm registry 和 PyPI wheels，不代理未知二进制。
2. 保留上游 checksum 与签名，内部镜像只做缓存。
3. 更新 `.cargo/config.toml`、`.npmrc` 与 `uv` 配置后重新生成所有锁文件。
4. 运行 `cargo deny check sources`、`pnpm check:supply-chain`、`pnpm audit` 和 Python `pip-audit`。
5. 在发布证据中记录镜像域名、锁文件 commit 与最终 artifact digest。
