# 跨平台方案

> 目标：Windows / macOS / Linux 上同一 Rust Core、同一 IR、同一契约；差异集中在路径、进程、沙箱、签名与安装器。

## 1. 支持矩阵

| 平台 | 架构 | 最低版本 | 支持级别 | 说明 |
| --- | --- | --- | --- | --- |
| macOS | arm64 + x86_64（universal） | macOS 13+ | 一级 | 签名/公证；容器可选 |
| Windows | x64 + arm64 | Windows 11（x64）/Windows 11 ARM | 一级 | Job Objects/AppContainer；签名安装器 |
| Linux | x86_64 + aarch64 | glibc 2.28+（Ubuntu 20.04+ / Debian 10+） | 一级 | Landlock/seccomp/cgroups v2；tar/deb/rpm |
| Linux（musl） | x86_64/aarch64 | Alpine 3.20+ | 二级 | 静态构建；部分 native 依赖需验证 |
| WASM 插件 | wasm32-wasip2 | — | 跨平台 | 由 Wasmtime 运行，不受 OS 限制 |

- 一级：CI 原生构建/测试/发布；二级：best-effort + 文档。
- 不承诺 32 位；不承诺 Windows Server 旧版本。

## 2. 构建矩阵

| 平台 | Runner | 目标 | 产物 |
| --- | --- | --- | --- |
| macOS arm64 | `macos-14`/arm64 | `aarch64-apple-darwin` | universal .app/.pkg/.tar.gz |
| macOS x64 | `macos-13` | `x86_64-apple-darwin` | 合并 universal |
| Windows x64 | `windows-2022` | `x86_64-pc-windows-msvc` | .msi/.zip |
| Windows arm64 | `windows-11-arm` | `aarch64-pc-windows-msvc` | .zip/msi（验证） |
| Linux x64 | `ubuntu-22.04` | `x86_64-unknown-linux-gnu` | tar.gz/deb/rpm |
| Linux arm64 | `ubuntu-22.04-arm` | `aarch64-unknown-linux-gnu` | tar.gz/deb/rpm |
| Linux musl | `ubuntu-22.04` + musl | `x86_64-unknown-linux-musl` | tar.gz（二级） |

- 使用 native runner，避免交叉编译 C 依赖（SQLite 用 `sqlite-bundled`，TLS 用 rustls）。
- macOS universal：分别构建 arm64/x64，再 `lipo` 合并；签名/公证在合并后。
- Windows ARM64：SQLite/openssl 依赖不可用；rustls + bundled SQLite 降低风险。

## 3. 平台差异

| 领域 | macOS | Windows | Linux |
| --- | --- | --- | --- |
| 数据目录 | `~/Library/Application Support` | `%LOCALAPPDATA%` | `~/.local/share` |
| 凭证 | Keychain | Credential Manager | Secret Service（无则加密文件） |
| 进程组 | `setpgid`/kill | Job Objects | `setpgid`/cgroup |
| 信号 | SIGTERM/SIGKILL | 终止进程/Job | SIGTERM/SIGKILL |
| 沙箱 | 容器优先；受限进程 | Job Objects/AppContainer | Landlock/seccomp/cgroups |
| 路径 | Unicode/大小写 | 长路径/保留名/大小写 | 大小写敏感/符号链接 |
| 安装 | .pkg/.dmg + 公证 | .msi + 代码签名 | deb/rpm/tar.gz |
| 字体 | Hiragino/Noto | Yu Gothic/Noto | Noto Sans JP/SC |
| 换行 | LF | CRLF（文本兼容） | LF |

## 4. 安装/更新

### macOS

- Universal `.pkg`/`.dmg`；Developer ID 签名 + notarytool 公证 + stapling。
- 更新：签名 manifest；下载 .pkg；安装需用户确认；保留上一版本回滚。
- 不在后台静默安装。

### Windows

- `.msi`（WiX）或签名 `.exe`；Authenticode 签名；可选 MSIX（企业）。
- 安装到 `%LOCALAPPDATA%` 或 Program Files（按范围）；用户级默认。
- 更新：签名 manifest；校验/回滚；不静默。

### Linux

- `.tar.gz` + `.deb` + `.rpm`；签名/校验和；systemd user service 可选。
- 不依赖系统 SQLite（bundled）；glibc 最低版本；AppImage 不作为一等（可后续）。
- 容器镜像：distroless/nonroot；SBOM/签名。

## 5. 本地运行时与可选依赖

- Rust 单二进制不依赖 Python/Node/JVM；核心 Workflow/Skill（WASM/内置）可用。
- Python AI Worker：需用户安装 3.13/uv；或用容器镜像；未安装则禁用 Agent。
- Node 脚本/插件：需 Node 26；或用容器；未安装则禁用 JS 脚本。
- Java（未来）：外部 JVM 进程；不打包。
- 模型：本地模型需用户安装/下载；不自动下载大模型。
- `hisha doctor` 检测平台/依赖/端口/沙箱/字体/模型，输出修复建议。

## 6. 沙箱

| 平台 | 默认 | 高风险 |
| --- | --- | --- |
| Linux | Landlock + seccomp + rlimit/cgroups | 容器/gVisor（可选） |
| macOS | 受限进程 + 路径/网络策略 | 容器（Docker/Podman） |
| Windows | Job Objects + restricted token/ACL | AppContainer/容器 |
| WASM | Wasmtime 能力沙箱 | 同左（无 OS 依赖） |

- 不可信插件默认 WASM；需要 native 的高风险插件默认容器。
- 无容器时 UI 明确风险；企业可强制“仅 WASM/容器”。

## 7. 字体/语言

- 日文：`"Noto Sans JP", "Hiragino Sans", "Yu Gothic", sans-serif`。
- 英文：`"Inter", "Noto Sans", sans-serif`。
- 中文：`"Noto Sans SC", "PingFang SC", "Microsoft YaHei", sans-serif`。
- 等宽：`"JetBrains Mono", "SFMono-Regular", "Cascadia Code", monospace`。
- 不打包商业字体；使用系统字体/开源 Noto；行高/字距按语言调整（见 CSS 架构）。
- 时区/日期/货币用 `Intl`；执行时间 UTC 存储、本地展示。

## 8. 测试

- CI 三平台：`cargo nextest`、`cargo clippy`、`cargo deny`；TS/Python 测试。
- 安装/卸载/升级/回滚 smoke；签名/公证验证。
- 路径/长路径/Unicode/符号链接/大小写。
- 进程终止/僵尸/临时目录清理。
- 沙箱行为（Linux 强；macOS/Windows 容器）。
- 性能基线（启动/内存/包体）。
- 本地 API/UI 607px/三语。

## 9. 风险

- macOS 公证/签名证书/CI secrets；Windows 代码签名成本。
- Windows ARM64 依赖兼容；Linux glibc 版本。
- macOS/Windows 沙箱弱于 Linux；容器依赖用户环境。
- 可选 Python/Node 版本差异；用户未安装。
- 大模型/依赖下载不可控；不自动。
- 更新失败/回滚；需双版本保留。

## 10. 相关文档

- [Local Runtime（Rust）](local-runtime-rust.md)
- [Script/Plugin 执行](script-plugin-execution.md)
- [Monorepo 与发布](monorepo-build-release.md)
- [部署架构](../02-architecture/deployment-architecture.md)
