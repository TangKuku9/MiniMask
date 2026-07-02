# 🦀 MiniMask

> 轻量、稳定、安全的内网穿透服务端，单二进制 + 嵌入式 Web UI。

MiniMask 采用 **单二进制 + 嵌入式前端 + 异步多路复用** 架构：一个 Rust 二进制同时提供
TLS 隧道服务端与图形化管理后台（Vue3 SPA 编译进二进制），零外部依赖即可部署。

## 特性

- **安全隧道**：TLS（rustls，ring 后端）+ yamux 多路复用，单条连接承载多路代理流。
- **Token 鉴权**：客户端使用一次性下发的 Token 连接，服务端只存哈希；可吊销/重置。
- **Web 管理后台**：嵌入式 Vue3 + TailwindCSS SPA，含仪表盘、客户端、端口映射、审计日志、设置。
- **实时监控**：WebSocket 推送带宽曲线、在线会话与连接统计。
- **端口映射热更新**：客户端在线时新增/删除/启停映射立即生效，无需重连。
- **审计日志**：登录、增删改、隧道连接等关键操作全部记录。
- **配套客户端**：同二进制 `client` 子命令，方便端到端测试与自部署。
- **双模式客户端**：命令行可用，双击 exe 也会自动弹出图形化客户端界面（egui，纯 Rust 内嵌）。

## 快速开始

```bash
# 1. 构建前端（首次）
cd web-ui && npm install && npm run build && cd ..

# 2. 构建并运行服务端（默认生成 config.toml 与自签证书）
cargo run --release
# 管理后台: http://localhost:8080  (默认 admin/admin，请尽快修改)
# 隧道端口: 0.0.0.0:7443 (TLS)

# 3. 在 Web UI 创建一个客户端，复制一次性 Token
#    然后运行配套客户端把某内网服务（例如本机 127.0.0.1:9000）暴露出去：
cargo run --release -- client --server 127.0.0.1:7443 --tls \
    --id <客户端ID> --token <Token> --server-name localhost

# 4. 在 Web UI 的「端口映射」新增：公网 18080 -> 本地 127.0.0.1:9000
#    立即可通过 http://<服务器IP>:18080 访问到内网服务。
```

## 子命令

| 命令 | 说明 |
| :--- | :--- |
| `minimask server [--config config.toml]` | 运行隧道服务端 + Web UI（默认） |
| `minimask client --server <host:port> --tls --id <id> --token <token>` | 运行隧道客户端 |
| `minimask gen-cert --out-cert cert.pem --out-key key.pem` | 生成自签证书 |
| `minimask hash-password <password>` | 生成 argon2 密码哈希 |
| `minimask gui`（或 `--gui`，或直接双击 exe） | 打开图形化客户端界面 |

## 图形化客户端

MiniMask 是**双模式程序**：

- **命令行执行**：从终端带参数运行时（如 `minimask client ...` 或 `minimask server`），行为与以往完全一致，日志输出到终端。
- **双击打开界面**：在资源管理器中直接双击 `MiniMask.exe`（无参数、非终端启动）时，程序会自动隐藏控制台窗口并打开图形化客户端界面。也可显式执行 `minimask gui` 或加 `--gui`。

图形界面（中文本地化、深/浅色主题）提供：

- **连接配置**：服务器地址、客户端 ID、Token（可切换显示/隐藏）、是否启用 TLS、Server Name（SNI）。
- **一键连接/断开**：后台自动重连，掉线不需手动干预。
- **实时状态**：未连接/连接中/已连接/重连中彩色徽章，显示运行时长与重连次数。
- **实时日志**：分级彩色日志、自动滚动、一键清空。
- **配置持久化**：上次填写的连接参数自动保存到 `data/client_gui.json`，下次一键连接。

> 双击检测原理：Windows 下用 `GetConsoleProcessList` 判断控制台是否仅挂载本进程，据此区分「双击启动」与「从终端启动」，双击时调用 `FreeConsole` 隐藏黑窗口。

## 架构

```
┌─────────────────────────────────────────────────────────┐
│                    Rust Tunnel Server                   │
│  ┌──────────────┐         ┌──────────────────────────┐  │
│  │  Axum Web UI │◄───────►│   Embedded SPA (Vue3)     │  │
│  │  (Port 8080) │  HTTP   │   Dashboard/Clients/...  │  │
│  └──────┬───────┘  WS     └──────────────────────────┘  │
│         │ State (Arc fields, RwLock/Mutex)               │
│  ┌──────▼───────┐         ┌──────────────────────────┐  │
│  │   Session    │  yamux  │   Tunnel Listener         │  │
│  │   Manager    │◄────────┤   (Port 7443, TLS+Yamux)  │  │
│  └──────┬───────┘  streams└──────────────────────────┘  │
└─────────┼─────────────────────────▲──────────────────────┘
          │ per visitor conn        │ TLS + Yamux
   ┌──────▼──────┐          ┌───────▼───────┐
   │  Public port│          │   Client      │
   │  listeners  │          │  (minimask    │
   │  :18080 ... │          │   client)     │
   └─────────────┘          └───────────────┘
```

### 目录结构

```
MiniMask/
├── Cargo.toml / build.rs / config.toml / Dockerfile
├── web-ui/                  # Vue3 + Vite + Tailwind 前端（构建产物被 rust-embed 嵌入）
└── src/
    ├── main.rs              # 入口：CLI 子命令、日志、jemalloc
    ├── config.rs            # TOML 配置加载/校验/默认生成
    ├── error.rs             # 统一错误类型 -> HTTP 响应
    ├── util.rs              # 密码哈希(argon2)、JWT、Token、自签证书、TLS 配置
    ├── state.rs             # AppState、客户端/会话/统计/审计/认证存储
    ├── server.rs            # 服务编排：启动隧道、Web、统计采样
    ├── client.rs            # 配套隧道客户端
    ├── web/                 # embed/auth/routes/ws/mod
    └── tunnel/              # protocol/listener/session/proxy/mod
```

### 协议

1. 客户端连接隧道端口（TLS），发送握手 `MMSK|ver|client_id|token`。
2. 服务端校验 Token（SHA-256 比对），返回 ok/fail。
3. 双方建立 yamux 会话（服务端 `Mode::Server`，客户端 `Mode::Client`）。
4. 访问者连接公网端口时，服务端打开一条 yamux 流并写入目标地址；客户端拨号本地服务后双向转发并实时计数字节。
5. 客户端断开时，代理通道关闭，公网监听器自动退出释放端口。

## 安全说明

- 隧道强制 TLS（自签证书首次自动生成；生产建议替换为正式证书或配置 mTLS）。
- 管理后台使用 JWT + HttpOnly + SameSite=Strict Cookie；如需远程访问请在配置中开启 `web_tls` 或置于反向代理之后。
- 客户端 Token 仅在创建/重置时明文返回一次，服务端只保存哈希。
- 连接数与并发客户端数可配置，防资源滥用。

## 配置示例（config.toml）

```toml
[server]
tunnel_bind = "0.0.0.0:7443"
web_bind    = "0.0.0.0:8080"
data_dir    = "./data"
tunnel_tls  = true
web_tls     = false          # 管理端口 HTTPS，默认关闭

[auth]
admin_username = "admin"
admin_password = "admin"     # 首次运行写入 data/auth.json（argon2 哈希）后请改密
jwt_secret = ""              # 留空则自动生成并保存到 data/jwt_secret
token_ttl_hours = 24

[security]
max_clients = 100
max_conns_per_client = 512
bw_limit_per_client = 0      # 0 = 不限速（预留）
```

## Docker

```bash
docker build -t minimask .
docker run -p 8080:8080 -p 7443:7443 -v $(pwd)/data:/app/data minimask
```

## 技术栈

Rust · tokio · axum · yamux · rustls(ring) · rcgen · rust-embed · serde/toml · tracing · clap · argon2 · jsonwebtoken · eframe/egui（GUI）｜ Vue3 · Vite · TailwindCSS
