# ADR-0022：企业传输、PKI 与 Secret Manager 边界

- 状态：Accepted（2026-11-01）
- 决策范围：Hybrid Worker、Connector、Artifact 和生产部署

## 背景

mTLS、证书轮换、HSM/Vault 和对象存储强依赖企业 CA、Secret Manager、S3/MinIO、DNS 与网络边界。把测试证书、明文环境密钥或本地文件系统伪装成生产能力，会破坏验收真实性。

## 决策

1. HPA 应用只接收短期文件/环境引用，不在源码中实现私有 Vault 协议。
2. Worker API 的 TLS/mTLS 在 Ingress/Service Mesh/反向代理终止；证书签发、吊销和轮换由企业 PKI 完成。
3. Connector 凭证通过 `credentialEnv` 引用；生产注入来源可为 Kubernetes Secret、Vault Agent 或云 Secret Manager。
4. S3-compatible Connector 已使用 SigV4；生产 endpoint、bucket policy、TLS 和 KMS 由部署环境提供。
5. 真实 mTLS、证书轮换和 Secret Manager 演练没有外部环境时为 `BLOCKED_EXTERNAL`。

## 必需生产控制

- Worker mTLS subject/SAN 绑定 tenant/controller 身份；
- 证书短期有效、自动续期、吊销传播和旧证书 fencing；
- 密钥不进入 Git、日志、Workflow、trace、Prometheus label 或 Artifact；
- Secret Manager 不可用时启动/Connector 操作 fail-closed；
- S3/KMS 权限最小化、版本/保留/删除和跨区域恢复经过演练。

## 当前状态

- 应用侧 HTTPS、S3 SigV4、凭证引用和脱敏已实现。
- 企业 mTLS PKI、证书轮换、真实 Vault/Secret Manager 与生产 S3 尚未提供环境证据，状态为 `BLOCKED_EXTERNAL`。
