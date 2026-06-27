# 🦀 Rust 高性能内网穿透服务端实现指南 (Vibe Coding Edition)

> **项目目标**：构建一个基于 Rust 的内网穿透服务端，支持浏览器访问特定端口打开图形化管理界面，具备连接方便、安全性好、稳定性高的特点。
> **适用场景**：个人/团队内网服务暴露、远程调试、私有云网关。

---

## 1. 项目架构设计

采用 **单二进制 + 嵌入式前端 + 异步多路复用** 架构，实现零依赖部署与高性能并发。

### 1.1 核心技术栈

| 模块 | 推荐库 | 选型理由 |
| :--- | :--- | :--- |
| 异步运行时 | `tokio` | Rust 生态事实标准，高并发稳定性极强 |
| 隧道协议 | `yamux` + `rustls` | TCP 多路复用 + TLS 1.3，稳定且安全 |
| Web 框架 | `axum` | 轻量模块化，原生支持 WebSocket 与静态资源 |
| 前端嵌入 | `rust-embed` | 将前端编译产物打包进二进制，单文件部署 |
| 前端 UI | `Vue3` + `Vite` + `TailwindCSS` | 现代化 SPA，开发体验好，构建产物小 |
| 配置管理 | `serde` + `toml` | 类型安全，人类可读的配置格式 |
| 日志监控 | `tracing` + `tracing-subscriber` | 结构化日志，支持异步追踪 |
| 内存分配 | `tikv-jemallocator` | 减少内存碎片，提升长期运行稳定性 |

### 1.2 架构图

```text
┌─────────────────────────────────────────────────────────┐
│                    Rust Tunnel Server                   │
│                                                         │
│  ┌──────────────┐         ┌──────────────────────────┐  │
│  │  Axum Web    │◄───────►│   Embedded SPA (UI)      │  │
│  │  (Port 8080) │  HTTP   │   - Dashboard            │  │
│  │              │  WS     │   - Client Management    │  │
│  └──────┬───────┘         │   - Port Mapping         │  │
│         │                 │   - Audit Logs           │  │
│         │ AppState        └──────────────────────────┘  │
│         │ (Arc<RwLock>)                                 │
│  ┌──────▼───────┐         ┌──────────────────────────┐  │
│  │  Session     │◄───────►│   Tunnel Listener        │  │
│  │  Manager     │  Event  │   (Port 443 / TLS+Yamux) │  │
│  │              │  Bus    │                          │  │
│  └──────────────┘         └──────────────────────────┘  │
│                                  ▲                      │
└──────────────────────────────────┼──────────────────────┘
                                   │ TLS + Yamux
                            ┌──────▼──────┐
                            │   Client    │
                            │  (Remote)   │
                            └─────────────┘

rust-tunnel-server/
├── Cargo.toml
├── config.toml              # 服务端配置文件
├── web-ui/                  # 前端项目（独立子目录）
│   ├── src/
│   ├── package.json
│   └── dist/                # Vite 构建产物（被 rust-embed 读取）
├── src/
│   ├── main.rs              # 入口：启动 Web + Tunnel 双服务
│   ├── config.rs            # 配置加载与校验
│   ├── state.rs             # 全局共享状态 AppState
│   ├── web/
│   │   ├── mod.rs           # Web 模块导出
│   │   ├── routes.rs        # RESTful API 路由
│   │   ├── auth.rs          # JWT / Token 认证中间件
│   │   ├── ws.rs            # WebSocket 实时事件推送
│   │   └── embed.rs         # rust-embed 静态资源服务
│   └── tunnel/
│       ├── mod.rs           # Tunnel 模块导出
│       ├── listener.rs      # TLS 监听 + Yamux 会话管理
│       ├── session.rs       # 客户端会话生命周期
│       └── proxy.rs         # 流量转发引擎（背压控制）
├── Dockerfile               # 多阶段构建
└── build.rs                 # 可选：自动触发前端构建


| 风险点 | 解决方案 | 实现方式 |
| :--- | :--- | :--- |
| 未授权访问 Web UI | JWT 认证 + HttpOnly Cookie | `axum-login` / 自定义中间件 |
| 未授权隧道连接 | Pre-shared Token / mTLS | 握手阶段验证，失败立即断开 |
| 中间人攻击 | 强制 TLS 1.3 | `rustls` 禁用旧版密码套件 |
| DDoS / 资源滥用 | 连接限流 + 带宽限制 | `tower-http` RateLimit + 令牌桶 |
| 敏感信息泄露 | 日志脱敏 + 环境变量注入密钥 | `tracing` filter + `dotenvy` |
| 内存安全 | Safe Rust + 定期审计 | `cargo audit` + `clippy` |
| 端口暴露 | 管理端口与隧道端口分离 | 分别绑定不同地址/端口 |



| 页面 | 核心功能 | 数据来源 |
| :--- | :--- | :--- |
| Dashboard | 实时带宽曲线、在线节点数、今日流量统计 | WebSocket `StatsUpdate` |
| 客户端管理 | 在线列表、Token 生成/吊销、连接时长、远程操作 | REST `/api/clients` + WS |
| 端口映射 | 可视化增删改查、协议类型、本地/远程端口、热更新 | REST `/api/mappings` |
| 审计日志 | 访问记录、异常告警、操作历史 | REST `/api/logs` |
| 系统设置 | TLS 证书管理、限流配置、用户管理 | REST `/api/settings` |

