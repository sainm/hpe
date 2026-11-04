# Phase 8 Hybrid Worker 私有部署与运维

> 本文面向在私有网络部署 `hpa worker` 的平台管理员。协议、租约与状态机以[Hybrid Worker 协议与执行契约](hybrid-workers.md)为准；控制面安装、企业模式、令牌与备份见[本地企业控制平面运维手册](enterprise-operations.md)。Phase 8 MVP 是单节点 SQLite 控制面，不提供 exactly-once、HA、mTLS 或自动扩缩容。

## 1. 架构与网络边界

```text
Worker 主机                                     控制面主机
┌──────────────────────────────┐                ┌─────────────────────────────────────┐
│ hpa worker                   │  HTTPS 443     │ reverse proxy（TLS termination）     │
│ - 不监听任何端口             │ ─────────────> │              │                      │
│ - 主动出站轮询               │                │              v 127.0.0.1:8787       │
│ - 同一 hpa-core Runtime      │ <───────────── │ hpa serve（单进程）                  │
└──────────────────────────────┘   JSON         │ control.db + tenants/{id}/hpa.db     │
                                                 └─────────────────────────────────────┘
```

- 控制面默认只监听 `127.0.0.1:8787`，本身不提供 TLS。远程 Worker 必须经反向代理或等价 TLS 终止层访问；控制面端口不要直接暴露到其他主机。
- Worker 不开放入站端口，不建立反向隧道，也不等待控制面连接。全部数据面请求由 Worker 主动发起：heartbeat、lease、start、events、complete。
- Worker 只访问控制面的 `/api/worker/*`。`/api/enterprise/*` 是管理员面，应由独立网络策略或反向代理规则限制来源，不向 Worker 主机开放。
- 本地同机部署可以使用 `http://127.0.0.1:8787`；只要控制面不是回环地址，Worker 就只接受 HTTPS。私有网络也不例外，明文 HTTP 会在启动时被拒绝。
- Worker 的 `--control-url` 不接受 userinfo（`https://user:pass@host`），不会跟随 HTTP 重定向，并显式禁用系统代理。需要出网代理的环境应使用网络层直连或透明转发，不能依赖 `HTTP_PROXY`/`HTTPS_PROXY`，也不要在 TLS 终止层改写证书。
- 请求超时为 15 秒。反向代理的读写超时应大于该值，且必须原样转发 `Authorization` 头与 JSON 请求体，保留 `401`、`409` 等状态码，不要替换成登录页或重定向。
- Worker 主机需能解析控制面主机名并完成证书校验。私有 CA 必须先安装到 Worker 主机的系统信任库；没有跳过证书校验的开关。NTP 仍需正常，以保证 TLS 证书有效期判断正确。
- 没有 mTLS。传输安全由 TLS 承担，应用认证只依赖 Worker token；因此反向代理前应再限制来源网段，不要把 Worker 端点暴露给任意互联网客户端。

`--control-url` 的校验结果：

| 输入 | 结果 |
| --- | --- |
| `http://127.0.0.1:8787`、`http://localhost:8787`、`http://[::1]:8787` | 允许 |
| `https://control.example.internal` | 允许 |
| `http://control.example.internal`、`http://10.0.0.10:8787` | 拒绝：明文只允许回环主机 |
| `https://user:secret@control.example.internal` | 拒绝：禁止 userinfo |
| `ftp://`、`ws://`、`file://` | 拒绝：只支持回环 HTTP 或 HTTPS |
| HTTPS 响应 301/302 | 不跟随，按失败处理 |

## 2. 角色、租户绑定与 token 生命周期

| 角色 | 凭证 | 能力 |
| --- | --- | --- |
| 平台管理员 | `hpa_ent_` token，且 `platformAdmin=true` | 注册 Worker、轮换/禁用 token、查看队列、派发/取消 Job |
| Worker daemon | `hpa_wrk_<base64url>` 一次性 token | heartbeat、lease、start、events、complete；仅限绑定租户 |
| 租户用户 | 现有租户 Bearer | 使用现有本地执行；Phase 8 的远程派发通过管理员或 Worker Center |

Worker token 的规则：

- 注册响应中的 `workerToken` 只在创建时显示一次；控制库只保存 SHA-256 哈希和 `worker_id`/`tenant_id` 绑定。列表、详情和审计都不会返回明文 token。
- token 与单个 Worker 记录、单个租户绑定。Worker 端点强制按 token 的 `tenant_id` 查询；跨租户 lease 不可见，跨租户上报返回 404/409。企业模式下 Worker token 不能调用管理员 API。
- 轮换调用 `POST /api/enterprise/workers/{id}/token`，旧 token 在同一事务内立即吊销，新 token 只显示一次。没有新旧 token 并存的重叠期。
- 禁用调用 `POST /api/enterprise/workers/{id}/disable`，会吊销该 Worker 的全部 token 并把状态置为 `disabled`。当前没有重新启用接口；需要重新注册一个 Worker。
- 删除租户会吊销该租户全部 Worker token；之后 token 一律 401。租户删除的完整行为见[企业运维](enterprise-operations.md#6-租户运维)。
- 轮换或禁用后，旧进程不会自动退出。它会在 heartbeat/lease 上持续收到 401 并按退避重试，必须更新 Secret 后重启服务。

> 实现提示：当前禁用接口只撤销 token 和置 `disabled`，不会主动取消该 Worker 的全部非终态 Job。已持有租约的 Job 在租约过期后回到 `queued`；派发时显式指定了该 `workerId` 且仍是 `queued` 的 Job 不会自动改派，需要取消后重新派发。

轮换操作应按以下顺序进行，避免长时间 401：

1. 调用轮换 API，立即把新 `workerToken` 写入目标 Worker 的 Secret 文件。
2. 确认文件权限为 `0600`。
3. `systemctl restart hpa-worker`，让进程重新读取环境变量。
4. 在 Worker Center 或列表接口确认新 heartbeat，再处理旧进程残留。

如果 token 可能泄露，先禁用 Worker 以立即阻断，再调查日志；不要只轮换而继续使用同一主机。

## 3. 部署与启动

### 3.1 前置检查与主机准备

控制面必须已以企业模式运行，`/api/health` 可访问且 `enterprise` 为 `true`。管理员 token 放入当前 shell 的 `$HPA_TOKEN` 时，避免命令历史记录：

```bash
read -rsp 'HPA_TOKEN: ' HPA_TOKEN; echo
export HPA_TOKEN
```

控制面与 Worker 必须使用同一版本、同一构建的 `hpa` 二进制。先在两侧比较：

```bash
hpa --version
curl -sS https://control.example.internal/api/health
```

`/api/health` 返回的 `version` 必须与 Worker 的 `hpa --version` 完全一致；不做 semver 兼容。Worker 主机先验证 DNS、TLS 和出站链路：

```bash
curl -sS https://control.example.internal/api/health
```

在 Worker 主机创建专用系统用户和目录。Worker 不需要数据目录、工作区或租户 SQLite；`/var/lib/hpa-worker` 只作为 systemd 的 `WorkingDirectory`，不会被 Worker 写入。

```bash
getent group hpa >/dev/null || sudo groupadd --system hpa
id hpa >/dev/null 2>&1 || sudo useradd --system --gid hpa --home-dir /var/lib/hpa-worker --shell /usr/sbin/nologin hpa
sudo install -d -m 0750 -o hpa -g hpa /var/lib/hpa-worker
sudo install -d -m 0750 -o root -g root /etc/hpa
sudo install -m 0755 -o root -g root target/release/hpa /usr/local/bin/hpa
```

防火墙只需允许 Worker 到控制面 TLS 端口的出站连接（通常 443）和 DNS；不要为 Worker 开放入站端口。

### 3.2 注册 Worker

用平台管理员 token 注册。`tenantId` 必须是已存在的 active 租户；`capabilities` 和 `maxConcurrency` 应与后续 daemon 参数一致：

```bash
curl -sS -X POST https://control.example.internal/api/enterprise/workers \
  -H "Authorization: Bearer $HPA_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"tenantId":"local","name":"worker-01","kind":"local","capabilities":["workflow"],"maxConcurrency":4}'
```

响应中的 `workerToken` 只出现一次，形如：

```json
{
  "id": "worker_...",
  "tenantId": "local",
  "name": "worker-01",
  "kind": "local",
  "status": "online",
  "capabilities": ["workflow"],
  "maxConcurrency": 4,
  "runtimeVersion": null,
  "workerToken": "<仅此一次显示；立即写入 Secret 文件>"
}
```

- `capabilities` 省略时默认为 `["workflow"]`；当前所有远程 Job 的 required capabilities 都是 `workflow`，所以 Worker 必须声明并实际发送 `workflow`。
- `maxConcurrency` 省略时为 1，注册和 heartbeat 都限制在 1 到 64。超过 64 会在注册时返回 422，或在 heartbeat 时返回 400 `WORKER_INVALID`。
- `metadata` 可省略，只能放非敏感的路由/位置标签；不要放 token、密码或连接器凭证。
- `status` 在刚注册时可能显示 `online`，因为注册会写入一次心跳时间；这不能证明 daemon 已启动，必须看 `lastHeartbeatAt` 是否持续前进。

在 Enterprise Center 的 Workers 页面也可以完成同样操作：选择租户，填写名称、kind、capabilities 与 maxConcurrency，注册后在一次性对话框中复制 token。列表不会再次显示 token；该页面也提供轮换、禁用、派发和取消。

### 3.3 写入 Secret 文件

```bash
sudo install -m 0600 -o root -g root deploy/hpa-worker.env.example /etc/hpa/worker.env
sudoedit /etc/hpa/worker.env
```

首次安装时使用上述命令；如果 `/etc/hpa/worker.env` 已存在，直接用 `sudoedit` 修改，避免把现有 token 覆盖为空。

文件内容至少包含：

```dotenv
HPA_WORKER_TOKEN=hpa_wrk_...
RUST_LOG=hpa=info
```

检查权限：

```bash
sudo stat -c '%a %U:%G %n' /etc/hpa/worker.env
```

期望为 `600 root root /etc/hpa/worker.env`。Worker 只从 `HPA_WORKER_TOKEN` 环境变量读取 token，没有 token 文件参数；systemd 的 `EnvironmentFile` 就是受保护的 Secret 文件。由于 systemd 以 PID 1 读取该文件，`0600 root:root` 不影响服务，不要为了手工启动而放宽到全局可读。若文件曾出现 group/other 读权限，应立即轮换 token。

不要把 token 写入 unit 的 `Environment=`、`ExecStart`、命令行参数、Shell 历史、CI 变量日志、工单或聊天记录，也不要放进 `--control-url` 的 userinfo 或 query。`env` 也不是远程派发字段。

### 3.4 安装并启动 systemd 服务

```bash
sudo install -m 0644 -o root -g root deploy/hpa-worker.service /etc/systemd/system/hpa-worker.service
sudo systemctl daemon-reload
sudo systemctl enable --now hpa-worker
```

单元文件的关键配置：

```ini
User=hpa
WorkingDirectory=/var/lib/hpa-worker
EnvironmentFile=-/etc/hpa/worker.env
ExecStart=/usr/local/bin/hpa --data-dir /var/lib/hpa-worker worker --control-url https://control.example.internal --max-concurrency 4 --capabilities workflow
Restart=on-failure
```

`EnvironmentFile` 前的 `-` 表示文件缺失时 systemd 仍会启动进程；但缺少 token 时进程会立即以 `HPA_WORKER_TOKEN is required` 退出，并触发有限次重启。正式环境应先确认文件存在，再 `enable`。

如果实际控制面主机名不同，修改 unit 中的 `--control-url` 后执行：

```bash
sudo systemctl daemon-reload
sudo systemctl restart hpa-worker
```

启动后检查：

```bash
systemctl status hpa-worker --no-pager
journalctl -u hpa-worker -f -o cat
sudo ss -ltnp | grep -F hpa || true
```

正常启动日志包含 `worker daemon starting`；成功租约时会看到 `worker leased job`、`worker job started`、`worker job completed`。`ss` 应没有该进程的监听端口。初始 heartbeat 失败只记录 warning 并继续运行，因此不能仅凭 `systemctl is-active` 判断 Worker 已连上控制面。

### 3.5 验证 heartbeat 与队列

查询 Worker 列表：

```bash
curl -sS "https://control.example.internal/api/enterprise/workers?tenantId=local" \
  -H "Authorization: Bearer $HPA_TOKEN"
```

重点字段：

- `status`：列表会把超过 30 秒没有心跳的 `active` 记录显示为 `offline`；数据库中的状态可能仍是 `active`。
- `lastHeartbeatAt`：应每 10 秒左右前进。等待 20 秒再查一次即可区分“刚注册”与“真实心跳”。
- `runtimeVersion`：daemon 首次 heartbeat/lease 后写入，必须与控制面版本一致。
- `capabilities`、`maxConcurrency`：heartbeat 会用 daemon 的参数覆盖注册值；以进程参数为准。

查询队列：

```bash
curl -sS "https://control.example.internal/api/enterprise/worker-jobs?tenantId=local" \
  -H "Authorization: Bearer $HPA_TOKEN"
```

`jobs` 中的 `status` 为 `queued`、`leased`、`running`、`waiting_approval`、`succeeded`、`failed` 或 `cancelled`。`attempts` 每次 lease 加一；`leaseExpiresAt` 是控制面时间；`cancelRequested` 表示已请求协作取消；`error` 保存 Worker 上报的失败原因。该列表最多返回 200 条；积压很大时只能看到最近 200 条，需要结合审计和 `control.db` 评估。

派发一个 smoke Job。先列出该租户的 Workflow 摘要，再使用实际 `workflowId`：

```bash
curl -sS "https://control.example.internal/api/enterprise/tenants/local/workflows" \
  -H "Authorization: Bearer $HPA_TOKEN"

curl -sS -X POST "https://control.example.internal/api/enterprise/worker-jobs" \
  -H "Authorization: Bearer $HPA_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"tenantId":"local","workflowId":"<workflowId>","input":{}}'
```

响应中的 `executionId` 可用于查询执行：

```bash
curl -sS "https://control.example.internal/api/executions/<executionId>" \
  -H "Authorization: Bearer $HPA_TOKEN"
```

如果 Job 一直 `queued`，按第 8 节检查版本、能力、`workerId` 绑定和 Worker 心跳。

## 4. 运行语义

| 项目 | 当前行为 |
| --- | --- |
| 租约 TTL | 30 秒（控制面常量） |
| heartbeat | 10 秒；续租 `activeJobs` 中未终态任务的 `leaseExpiresAt` |
| 空闲 lease 轮询 | 1 秒 |
| HTTP 请求超时 | 15 秒 |
| 上报重试 | 首次之外最多重试 8 次，退避 1、2、4、8、16、30、30、30 秒；仅网络错误、5xx、408、429 和无效 JSON 重试，401/409/其他 4xx 不重试 |
| 并发 | `--max-concurrency` 取 1 到 64；本地按活动 Job 数占用槽位，控制面再按注册的 `max_concurrency - leased/running` 限制 |

- 每个 Job 在 Worker 上是一个独立 tokio task。heartbeat 和 lease 循环独立运行；单个 Job 的上报失败不会停止其他 Job。
- 控制面只在 `queued` 中挑选属于该 Worker 租户、未被其他 Worker 固定、版本完全匹配、能力满足的 Job。一次 lease 的 `attempts` 加一，并生成新的 `lease_id`。
- 并发槽只统计 `leased`/`running`。`waiting_approval` 不占用槽；审批后重新入队时才重新消耗槽位。
- Worker 的 `capabilities` 必须在“注册声明”和“每次 lease 请求”两处都包含 Job 的 required capabilities。当前 required capabilities 固定为 `workflow`；能力不足时 Job 保持 `queued`，不会返回错误。
- 版本按 `CARGO_PKG_VERSION` 精确匹配。控制面派发时写入自己的版本，lease 只选择同版本 Worker；daemon 在执行前还会再校验一次，不一致时以 `runtime-version-mismatch` 失败上报。
- 派发时可以指定 `workerId` 固定初始执行者；不指定时租户内任意满足版本和能力的 Worker 都可以领取。审批重新入队和租约过期重排都会清空 `worker_id`，因此后续 attempt 可能落到另一台 Worker。
- 状态机：

```text
queued -> leased -> running -> succeeded | failed
                             -> waiting_approval -> queued -> ...
queued / leased / waiting_approval -> cancelled
leased / running 且租约过期 -> queued（若已请求取消则为 cancelled）
```

### 4.1 审批

Worker 遇到 `human.approval` 时以 `waiting_approval` complete，控制面清空 lease、释放并发槽，Job 进入 `waiting_approval`。审批 API 保存新的 ExecutionState 后，如果存在对应远程 Job，会把 Job 置为 `queued`、`resumed=true`、清空 `worker_id` 和 lease，不触发本地 `RuntimeManager`。Worker 下一次 lease 从新状态继续，不再发送 `execution.started`。

控制面后台每秒扫描远程 `waiting_approval`；到 SLA 后按节点 `onTimeout` 生成恢复状态、写入租户事件并把 Job 重新置为 `queued`。也可提前在 Approval Center 人工处理。长期停留在审批的 Job 不占 Worker 槽。

### 4.2 取消

- `queued`、`leased`、`waiting_approval`：取消 API 立即把 Job 置为 `cancelled` 并清空 lease；已持租约的 Worker 后续上报会得到 409。
- `running`：只做协作取消：控制面设置 `cancel_requested`，heartbeat/lease 响应带回 `cancelJobs`；Worker 在 step 之间检查取消标志，并将状态 complete 为 `cancelled`。一次正在进行的 HTTP 调用无法强杀，属于 best effort；退避 sleep 可被取消标志打断。
- 取消与完成存在竞态：如果 Worker 在收到 `cancelJobs` 前已经完成并上报 `succeeded`，控制面会保留 `succeeded`。这是协作取消的预期限制，不应把取消接口视为强制终止。
- 如果 Worker 离线或长时间不检查心跳，租约过期时 `cancel_requested=1` 的 Job 会直接转为 `cancelled`，不会重新入队。
- 远程任务应使用 `/api/enterprise/worker-jobs/{id}/cancel`；`/api/executions/{id}/cancel` 在 `running` 时仍会返回 `CANCEL_RUNNING`，不要用它替代 Worker 取消接口。

### 4.3 租约过期、重复执行与 409

- 过期租约在控制面下一次处理 lease 时原子清理；管理员取消 Job 时也会先触发清理。没有独立的后台定时清理任务：如果所有 Worker 都停止轮询，过期 Job 会保持 `leased/running`，直到有 Worker 再次 lease 或管理员操作。
- Worker 断线、进程被杀、事件循环阻塞或控制面不可达超过 30 秒时，租约可能过期并重排；旧 lease 的 start/events 以及未终态 Job 的 complete 会收到 409 `WORKER_CONFLICT`（lease is stale），daemon 记录 `worker job lease is no longer valid; abandoning job` 并丢弃该 Job，而不是重试。
- `attempts` 会随每次重排增加。同一 execution 可能被执行多次，HTTP 写操作、外部脚本或业务副作用必须由业务侧幂等键/去重来保证。**本 MVP 是至少一次语义，不承诺 exactly-once。**
- 上报本身有重试；如果事件已经写入但响应丢失，Worker 可能重复发送同一批事件。控制面按 sequence 幂等写入事件，但外部副作用不受此保护。
- 如果 start、events 或 complete 在 8 次重试后仍失败，daemon 放弃该 Job 并移除本地活动记录；租约过期后控制面重排，旧结果被丢弃。
- 不要在同一个 Worker token 上启动两个 daemon。它们会共享同一 Worker 记录和租约容量，容易出现容量误判、重复执行和 409。

## 5. systemd 单元、加固与平台差异

部署文件：

- [hpa-worker.service](../../deploy/hpa-worker.service)
- [hpa-worker.env.example](../../deploy/hpa-worker.env.example)

单元的关键约束：

| 配置 | 作用 |
| --- | --- |
| `User=hpa` / `Group=hpa` | 非 root 专用账户运行 |
| `WorkingDirectory=/var/lib/hpa-worker` + `StateDirectory=hpa-worker` | 保存 `worker-outbox.db`；目录由 systemd 创建并按 `0700` 管理 |
| `EnvironmentFile=-/etc/hpa/worker.env` | 从 0600 Secret 文件读取 token 与日志级别 |
| `ExecStart=/usr/local/bin/hpa --data-dir /var/lib/hpa-worker worker ...` | 持久 outbox 目录、控制面 URL、并发和能力 |
| `Restart=on-failure` / `RestartSec=5s` | 崩溃或非零退出后重启；配合 `StartLimitBurst=5` 防止死循环 |
| `NoNewPrivileges=yes` | 禁止提权 |
| `PrivateTmp=yes` | 私有 `/tmp`，不暴露宿主机临时目录 |
| `PrivateDevices=yes` | 不访问不需要的设备节点 |
| `ProtectSystem=strict` | 整个文件系统只读，仅保留 systemd 提供的 API 文件系统 |
| `ProtectHome=read-only` | 不写入用户目录 |
| `ProtectKernelTunables/Modules`、`ProtectControlGroups` | 限制内核与 cgroup 修改 |
| `RestrictRealtime`、`RestrictSUIDSGID`、`LockPersonality` | 限制实时调度、setuid/setgid 文件和个人性设置 |
| `CapabilityBoundingSet=` / `AmbientCapabilities=` | 清空能力集 |
| `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK` | 只保留 DNS、TCP、Unix socket 和 netlink 所需地址族 |
| `StandardOutput=journal` / `StandardError=journal` | 日志进入 journal，不写本地文件 |

- Worker 通过 `--data-dir /var/lib/hpa-worker` 把持久 outbox 放在 systemd 的 `StateDirectory`；不要复用控制面数据目录，也不要给整个 `/var/lib` 写权限。
- `RUST_LOG=hpa=info` 适合生产。临时使用 `hpa=debug` 可以观察重试细节，但不要长期开启；日志中不应出现 token，若出现应立即轮换。
- 当前 Workflow 的 `script` 节点是进程内 jq 表达式，不是 shell；Worker 不提供任意宿主机命令接口。`http.request` 节点会从 Worker 主机发起出站请求，受 Workflow 节点既有 Runtime 网络权限约束，但仍应在主机防火墙上限制其可达网段。
- 单元没有启用 `RestrictNamespaces` 或 `MemoryDenyWriteExecute` 等会限制执行器/运行时的选项，以免影响后续 Runtime 能力。若自行加严，必须重新验证所有 Workflow 节点类型。
- macOS 可以使用 launchd 运行同一 CLI，将 token 放在 `0600` 文件并通过包装脚本注入环境变量，使用 `KeepAlive` 重启；本阶段不提供 launchd 模板。Docker 部署不在本阶段范围内。

## 6. 升级、排空与恢复

### 6.1 版本规则

控制面、所有 Worker（包括内置 Registry/节点实现）必须来自同一版本构建。`GET /api/health` 的 `version` 与 Worker 的 `hpa --version` 必须完全相同。混用版本的直接结果：

- 旧 Worker 不会领取新控制面创建的 Job（lease 查询按 `runtime_version` 精确过滤）。Job 列表不返回 `runtimeVersion`，遗留任务可用创建时间或只读查询 `control.db` 的 `enterprise_worker_jobs.runtime_version` 判断。
- 新 Worker 不会领取旧控制面创建的 Job；控制面升级后，数据库中遗留的旧版本 `queued` Job 可能永远无法被领取。
- 已经领取的 Job 若 daemon 版本不匹配，会以 `runtime-version-mismatch` 失败上报，而不是继续执行。

因此升级前必须排空队列，升级后取消并重新派发所有遗留的旧版本 Job。

### 6.2 计划升级与排空

当前 daemon **没有** `--drain` 参数，也没有在 SIGTERM 时等待当前 Job 完成的逻辑；`systemctl stop` 会直接终止进程，在途 Job 立即丢失，只能等租约过期后重排。可执行的“排空”流程是：

1. 停止新的派发（API 调用方、Worker Center、自动化任务）。
2. 先处理或取消所有 `waiting_approval`，再轮询 `/api/enterprise/worker-jobs?tenantId=...`，直到没有 `queued`、`leased`、`running`；不要把审批中的 Job 留到版本切换之后。
3. 逐个 `systemctl stop hpa-worker`。多台 Worker 在同一版本内可以逐个重启；跨版本不能滚动混跑。
4. 停止控制面，备份 `control.db`，再替换控制面二进制。
5. 用同一构建替换所有 Worker 的 `/usr/local/bin/hpa`。
6. 先启动控制面并检查 `/api/health`，再启动 Worker，确认 heartbeat、版本、能力。
7. 用一个 smoke Workflow 验证派发、执行、事件和结果，再恢复派发。

如果无法等待排空而强制停止 Worker：

- 在途 Job 的租约最多保持 30 秒；控制面下一次 lease 时将其重排。
- 如果所有 Worker 同时停止，控制面不会主动扫描过期租约；需要至少一个 Worker 恢复轮询或管理员先触发取消，过期清理才会发生。
- 旧 Worker 的迟到上报会得到 409；业务侧可能已经产生副作用，需要按幂等键检查重复。
- 不要用“禁用 Worker”代替排空：禁用会立即吊销 token，已运行 Job 的上报变成 401，且不会优雅完成。

### 6.3 滚动升级的现实边界

- SQLite 单节点模式没有多节点滚动升级；跨版本升级必须有停机窗口。
- `HPA_POSTGRES_RUNTIME=1` 的多控制面模式支持同版本滚动：控制面逐个替换、Worker 执行中强杀并重启后任务可重派。`pnpm test:worker-rolling` 使用两个共享 PostgreSQL 的控制面、两个 Worker 和故障切换代理验证 20 项检查，见 [Phase 27](../11-roadmap/phase-27-worker-rolling-upgrade.md)。
- 多个 Worker 只有在已经与目标控制面同版本时，才能逐个重启实现“滚动”；先升级部分 Worker、再升级控制面会产生一段无法调度的窗口。
- 跨版本混跑仍不支持：旧 Worker 不会领取新控制面 Job，反之亦然；升级 Registry/节点实现时，Job 快照中的 Workflow 仍由 Worker 本地 `hpa-core` 编译，版本不一致会表现为无 Job 或执行失败，而不是自动降级。

### 6.4 备份与恢复

- `control.db` 包含 Worker 注册、token 哈希、Job 状态、租约和审计，但不包含 Worker daemon 的本地 outbox。
- Worker 的已生成 start/events/complete 先落本地 SQLite；进程崩溃重启后继续发送。尚未生成报告的在途执行仍按至少一次语义从租约重排。
- 备份 `control.db` 时先停止控制面，再执行：

```bash
sqlite3 "$HPA_DATA/control.db" ".backup '/safe/control-$(date +%F).db'"
```

- 租户业务库通过控制面的备份 API 单独备份；控制库不在租户备份范围内。完整备份命令与恢复后重启要求见[企业运维](enterprise-operations.md#8-备份恢复与保留)。
- 恢复 `control.db` 会回退 Worker 注册、token 哈希、Job 与租约状态。恢复前停止所有 Worker；恢复后轮换 Worker token、更新 Secret 并重启，核对并清理与租户库不一致的 Job。
- 恢复租户 `hpa.db` 前先停止 Worker 并取消该租户的非终态远程 Job，避免控制面的 Job 状态与租户 execution 状态分叉；恢复后按控制面要求重启 `hpa serve`，再重新派发。
- Broker、Redis 或外部 outbox 均不存在；不要假设控制库和 Worker 之间有事务性消息队列。

### 6.5 控制面与 Worker 重启

- 控制面重启：Worker 继续 heartbeat/lease，网络错误按 1 到 30 秒退避；控制面恢复后自动继续。`remote_executions` 会阻止本地 Runtime 恢复/审批扫描接管远程 execution。服务重启期间过期的租约在下次 lease 时重排。
- Worker 重启：新进程先排空本地 outbox；控制面仍只接管成功上报的状态。未完成执行在租约过期后重排。
- 网络中断：heartbeat 和 lease 循环分别退避重试，进程不会因短暂断线退出。上报最多重试 8 次；租约过期后旧上报 409，Job 重排。恢复网络后不需要手工重启，但如果 token/证书已变更则需要重启加载新配置。
- 审批等待中的 Job 在 Worker 重启后不需要重新执行到审批点：状态已保存在租户库和控制库，审批后重新入队即可。

## 7. 安全基线

### 7.1 Worker 主机隔离

- 不需要挂载控制面数据目录、租户 SQLite 或任何租户工作区。Worker 只执行控制面下发的 Workflow 快照，不直接访问租户数据库。
- 使用专用非 root 用户、无登录 shell、只读根文件系统和私有 `/tmp`；不要在 Worker 主机放置控制面备份、连接器凭证或平台管理员 token。
- Worker 不监听端口，不需要入站规则；出站只放行控制面 TLS 端点、DNS 和 Workflow 实际需要的目标。建议在主机防火墙上阻断云元数据地址和不需要的内网网段，避免 `http.request` 节点成为内网探测通道。
- Worker 主机上的 Workflow 执行仍属于不可信任务边界：不要与高价值数据库、控制面进程或 Docker socket 共置；需要更强隔离时使用独立主机/网络分区。

### 7.2 执行边界

- Worker 只执行 Workflow IR 节点，不提供远程 shell 或任意宿主机脚本接口。当前 `script` 节点是 jq 表达式；`http.request` 节点按既有 Runtime 网络权限发起请求。
- 控制面不向 Job 注入连接器凭证；`env` 不是远程派发字段。连接器凭证仍留在租户/连接器配置中。
- Job 的 `workflow_json`、`state_json` 和 output 会持久化在控制库/租户库，可能包含业务输入输出。不要把未经审计的 secret 放进 Workflow 输入或节点配置；远程 Job 不是 Secret 传递通道。

### 7.3 Token、TLS 与文件

- Worker token 只能来自环境变量，生产通过 `0600` 的 systemd `EnvironmentFile` 注入；不要写入 `ExecStart`、unit 的 `Environment=`、metadata、日志、工单或代码库。
- 每个 Worker 使用独立 token；不要在多台主机或多个 daemon 间复用。轮换/禁用/租户删除后，旧 token 立即失效。
- 控制面保持回环监听，远程访问只经 TLS 反向代理；代理限制 `/api/worker/*` 来源，保留 `Authorization`，不要缓存或记录请求头。
- 控制面数据目录 `0700`，`control.db` 只对服务账户可读写；备份文件按敏感数据保护。Worker token 在控制库中只有 SHA-256 哈希。
- 生产日志使用 `RUST_LOG=hpa=info`。日志不会主动打印 token，但错误体摘录和 Job 状态可能包含业务信息；日志访问权限按敏感数据管理。发现 token 出现在日志、进程参数或工单中，立即轮换或禁用。

| 路径 | 建议权限 |
| --- | --- |
| `/usr/local/bin/hpa` | `0755 root:root` |
| `/etc/hpa` | `0750 root:root` |
| `/etc/hpa/worker.env` | `0600 root:root` |
| `/etc/systemd/system/hpa-worker.service` | `0644 root:root` |
| `/var/lib/hpa-worker` | `0750 hpa:hpa`，仅作工作目录 |
| 控制面数据目录 | `0700`，仅控制面服务账户 |

## 8. 监控与故障排查

本 MVP 没有 Prometheus/metrics 端点，也没有内置告警。监控由 systemd、journal 和管理 API 轮询组成。建议至少观察：

| 信号 | 方法 | 说明 |
| --- | --- | --- |
| 进程状态 | `systemctl is-active hpa-worker`、`systemctl show -p NRestarts` | 进程 active 不代表已连上控制面 |
| 心跳年龄 | `GET /api/enterprise/workers` 的 `lastHeartbeatAt` | 超过 30 秒列表显示 `offline`；检查网络、TLS、token |
| 队列积压 | `GET /api/enterprise/worker-jobs` 按 status 统计 | 关注 `queued` 数量、最老 Job 年龄、`attempts` 增长 |
| 审批积压 | `GET /api/approvals` 或 execution 状态 | 先确认 SLA 是否已到；控制面会自动超时恢复，也可人工提前处理 |
| 租约/重试日志 | `journalctl -u hpa-worker` | 401、409、`backing off`、`lease is no longer valid` |
| 控制面容量 | `control.db` 大小、磁盘、进程连接数 | 单节点 SQLite，容量需人工规划 |

故障排查：

| 现象 | 检查 | 处理 |
| --- | --- | --- |
| Worker 日志 `HTTP 401`，heartbeat/lease 持续退避 | token 是否为空、含换行、已被轮换/禁用；租户是否已删除；反向代理是否剥离 `Authorization`；`EnvironmentFile` 是否被实际加载 | 写入正确的新 token 并重启；被禁用则重新注册 Worker；检查代理头转发。不要把 token 打到终端或工单 |
| 日志 `lease is no longer valid; abandoning job` 或 API 409 `WORKER_CONFLICT` | 是否同一 token 跑了多个 daemon；是否网络中断/进程暂停导致租约超过 30 秒；是否在上报重试期间被重排 | 保证一 token 一 daemon；恢复网络；接受该 Job 已在别处重跑，检查业务幂等；无需重试旧 lease |
| Worker 在线但一直无 Job | Worker 的 `runtimeVersion`、`capabilities`、`tenantId`；Job 的 `workflowVersion`、`workerId`、`status`、创建时间；是否被固定到离线 Worker | 对齐版本；确保 daemon 和注册都包含 `workflow`；取消固定绑定或重新派发；确认队列中确实有 `queued` Job |
| Job 因版本不匹配无法领取，或错误 `runtime-version-mismatch` | `/api/health` 的 `version`、`hpa --version`；Job 创建时间是否早于控制面升级；必要时只读查询 `control.db` 的 `enterprise_worker_jobs.runtime_version` | 控制面与全部 Worker 换同一构建；旧版本队列 Job 取消后重新派发，不要混跑 |
| Worker 显示 `online` 但 capabilities mismatch | 注册值和 `--capabilities` 是否一致；heartbeat 是否已覆盖注册值；当前 Job 的 required capabilities 是否为 `workflow` | 修改 unit 参数并重启；确保 daemon 的列表包含 `workflow`；确认没有空的 capability |
| Worker 显示 `offline` | 30 秒内是否有 heartbeat；journal 是否持续 `heartbeat failed`；DNS/TLS/CA/时间/防火墙；控制面是否重启 | 修复网络或证书；token 失效则轮换；进程未退出但一直失败时不要只看 systemd 状态 |
| `queued` 持续增长 | 在线 Worker 数量、每台 `maxConcurrency`、是否被离线 Worker 固定、是否有大量长任务占满槽位 | 手工增加 Worker；取消固定绑定/重新派发；本 MVP 没有自动扩缩容 |
| `waiting_approval` 长时间停留 | Approval Center/`GET /api/approvals`；该 execution 的 `waiting`；Worker 是否已释放槽位 | 人工审批或取消；审批后 Job 会变 `queued`/`resumed=true`，需有匹配 Worker 才能继续；远程审批不会自动超时 |
| 取消后仍显示 `running` | `cancelRequested` 是否为 true；Worker 是否在线；当前是否卡在长 HTTP 调用；是否有 heartbeat | 等待 step 边界或租约过期（过期时 `cancel_requested=1` 会直接 cancelled）；必要时重启 Worker，注意副作用 |
| heartbeat 返回 400 `WORKER_INVALID` | `--max-concurrency` 是否在 1 到 64；capabilities 是否有空字符串 | 修正 unit 参数并重启；不要用 0 或大于 64 的值 |
| 注册返回 422 `OPS_INVALID` | 名称/kind 是否为空；maxConcurrency 是否越界；capabilities 是否为空 | 修正请求；租户不存在返回 404，非平台管理员返回 403 |

常用排查命令：

```bash
systemctl status hpa-worker --no-pager
journalctl -u hpa-worker --since '30 min ago' -o cat
journalctl -u hpa-worker -o cat | grep -E 'HTTP 401|HTTP 409|lease is no longer valid|backing off'
curl -sS https://control.example.internal/api/health
```

不要把完整 token 放进排查命令；需要比较时只检查前缀 `hpa_wrk_`、长度和更新后的文件权限。

## 9. 明确边界

| 边界 | 说明 |
| --- | --- |
| 控制面 | 单节点 SQLite；无 Redis/Kafka、无 HA、无多节点、无在线迁移和故障切换；不要把数据目录放到 NFS 或由多个进程共享 |
| 传输与身份 | 无 mTLS、无反向隧道；TLS 反向代理 + Bearer Worker token；没有浏览器 SSO 登录 Worker |
| 调度 | 无自动扩缩容、无优先级/抢占、无全局调度器；容量靠人工增加 Worker |
| 凭证与产物 | 无远程凭证注入/Vault 集成；无对象存储；产物写回租户库；`env` 不作为派发字段 |
| 审批 | 控制面自动执行远程审批 SLA，也可人工提前处理 |
| 交付语义 | 有本地 durable outbox；至少一次，不承诺 exactly-once |
| Worker 本地状态 | 只有 outbox SQLite，无租户 SQLite/工作区访问 |
| 排空 | 当前无 `--drain`/SIGTERM 优雅排空；停止进程即中止在途任务，靠租约过期重排 |
| 平台 | Docker 不在本阶段；macOS launchd 仅可自行适配，不提供模板；无 Windows 服务 |

## 10. 验收

以下清单用于私有部署验收；每项以实际命令和实际运行结果为准，本文不声明通过数量、不伪造运行结果。

| 验收项 | 操作 | 期望 |
| --- | --- | --- |
| 真实 daemon 进程 | 按第 3 节安装并 `systemctl start hpa-worker` | 进程 active；journal 出现 `worker daemon starting`；控制面列表的 `lastHeartbeatAt` 持续前进 |
| heartbeat/queue | 查询 workers 与 worker-jobs | Worker `runtimeVersion` 与控制面一致；队列能看到 Job 状态流转；无监听端口 |
| 普通执行 | 派发一个可成功的 Workflow | Job 最终 `succeeded`；租户库 execution/events/artifact 正确；日志出现 leased/started/completed |
| 审批恢复 | 派发含 `human.approval` 的 Workflow，等待 `waiting_approval` 后审批 | Job 从 `waiting_approval` 变 `queued`、`resumed=true`、`attempts` 增加；最终成功；不重复发送 start 事件 |
| 取消 | 在 `waiting_approval` 或 `running` 时调用 worker-jobs cancel | waiting/queued/leased 立即 `cancelled`；running 在 step 边界协作取消并 complete；日志出现 cancellation requested |
| 租约过期 | Job running 时停止 Worker，等待超过 30 秒，再启动 Worker | 过期后 Job 回到 `queued`、`attempts` 增加；新 Worker 可完成；若旧进程仍存活并在过期后上报，journal 出现 409/abandon |
| token 生命周期 | 轮换 token，旧进程继续运行；再禁用 Worker | 旧 token 立即 401；新 token 重启后可用；禁用后不再 heartbeat/lease；列表不返回 token |
| 版本/能力过滤 | 使用不匹配版本或缺少 `workflow` 的 Worker | 不领取 Job；修正版本/能力后可领取；无静默跨版本执行 |
| 跨租户隔离 | 用租户 A 的 Worker token 查询/上报租户 B | 不可见/404/409；不能跨租户 lease |
| 重启恢复 | 重启控制面；重启 Worker；断开网络后恢复 | 本地 Runtime 不接管远程 execution；Worker 先排空 outbox；未完成执行靠过期租约重排；外部副作用仍需业务幂等 |

可运行现有测试作为回归，但测试通过不等于私有环境验收通过：

```bash
cargo test -p hpa-local --test enterprise worker_daemon_executes_remote_workflow_and_resumes_approval
cargo test -p hpa-local worker::tests
```

集成测试覆盖真实 daemon 进程、审批恢复和取消等待中的 Job；单元测试覆盖租约过期、轮换/禁用、跨租户过滤和 409。实际部署仍需在目标网络、TLS 代理和 systemd 环境中按上表手工验证，尤其是真实进程被杀后的租约重排和审批恢复。

## 11. 已知实现差异

以下按当前代码确认，若与协议概述不一致，以本节和实际行为为准：

- 协议文档提到 `--drain`/SIGTERM 优雅排空；当前 CLI 只有 `--control-url`、`--max-concurrency`、`--capabilities`，没有 `--drain`，进程也未处理 SIGTERM 等待在途 Job。停止服务即中止任务，依赖 30 秒租约过期重排。
- Worker 禁用只吊销 token、置 `disabled`，不主动取消 Job；运行中 Job 由租约过期重排，固定到该 Worker 的 `queued` Job 需要手工处理。租户删除会取消可列出的非终态 Job，但该路径只遍历最近 200 条 Job（`list_jobs` 上限），更早的记录可能保留；删除前仍应先排空队列。
- 过期租约不是后台定时扫描，而是在下一次 lease 或管理员取消时惰性清理；所有 Worker 离线时不会自动重排。
- Worker 只从 `HPA_WORKER_TOKEN` 环境变量读取 token，没有独立的 token 文件参数；`EnvironmentFile` 是受支持的 Secret 文件方式。
- CLI 的 `--max-concurrency` 只做下限 1 的修正，没有上限修正；控制面注册和 heartbeat 限制 1 到 64。传入大于 64 会导致 heartbeat 400，lease 也可能继续使用旧的注册值。请始终配置 1 到 64。

## 12. 相关文档

- [Hybrid Worker 协议与执行契约](hybrid-workers.md)
- [本地企业控制平面运维手册](enterprise-operations.md)
- [本地企业实现与安全模型](local-enterprise.md)
- [执行平面](../02-architecture/execution-plane.md)
- [API 规范](api-specification.md)
- [测试策略](../10-quality/test-strategy.md)
- [hpa-worker.service](../../deploy/hpa-worker.service)
- [hpa-worker.env.example](../../deploy/hpa-worker.env.example)
