# lanclip

> 局域网剪切板与文件互传。极数本源出品，来自 [API Zero](https://apizero.cn/) 免费项目。

lanclip 是一个 macOS / Windows 桌面工具，用于在局域网设备之间同步剪切板历史、文本和图片，并提供菜单栏快速搜索面板与 GPUI 控制台。

品牌图标、菜单栏图标和控制台图标已内嵌到 release 二进制中，发布包会同时附带 `lanclip.svg`。

## 快捷键

- macOS：`Cmd + Shift + V`
- Windows：`Ctrl + Shift + V`

按下快捷键会打开或隐藏剪切板历史面板；点击条目会写入系统剪切板。

## 设计

详见 [`设计文档.md`](./设计文档.md)（v0.3：事件驱动 + 严格防回环 + 多连接并发）。

## 技术栈

- **GUI**: `gpui` + `gpui-component`
- **异步**: `tokio` (multi-thread)
- **传输**: `tokio-tungstenite`（WebSocket，1 控制 + N 数据连接）
- **发现**: `mdns-sd`（独有服务类型 `_lanclip._tcp.local.`）
- **剪切板**: `clipboard-rs`（平台原生事件，非轮询）

## 构建

```bash
cargo check --workspace
cargo run -p lanclip-ui --bin lanclip
cargo build --release -p lanclip-ui --bins
```

GitHub Actions 会在推送 tag `v*` 时自动构建：

- Windows x64
- macOS Apple Silicon
- macOS Intel

## Workspace 结构

```
crates/
├── lanclip-domain     # 纯模型
├── lanclip-proto      # Msg 枚举 + JSON 编解码
├── lanclip-discovery  # mDNS
├── lanclip-network    # WebSocket + 连接池
├── lanclip-clipboard  # 监听 + 防回环
├── lanclip-transfer   # 多文件并发传输
├── lanclip-app        # 服务编排（Application 层）
└── lanclip-ui         # gpui 桌面端（binary）
```

## 路线图

详见 [`设计文档.md` §12](./设计文档.md#12-开发路线图)。

## 状态

MVP：菜单栏剪切板历史、全局快捷键、局域网发现、GPUI 控制台和 GitHub Actions 自动构建。
