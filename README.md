# lanclip

> macOS / Windows 局域网剪切板历史、文本同步、图片同步与文件互传工具。  
> 极数本源出品，来自 [API Zero](https://apizero.cn/) 免费项目。

lanclip is a lightweight LAN clipboard manager and clipboard sync app for macOS and Windows. It provides a macOS-style menu bar clipboard history, global clipboard shortcut, local network discovery, trusted device pairing, image/text clipboard preview, and a modern GPUI control console.

## 为什么做 lanclip

在多台电脑之间工作时，最常见的痛点不是“传大文件”，而是这些小动作太频繁：

- 在一台电脑复制文本，另一台电脑也想直接粘贴。
- 复制过的内容想快速搜索、预览、再次使用。
- 图片、链接、验证码、代码片段需要在局域网内快速流转。
- 不想依赖云同步，也不想为了临时剪切板内容打开复杂工具。

lanclip 的目标是做一个本地优先、局域网优先、响应足够快的剪切板互传工具。

## 核心功能

- 菜单栏剪切板历史面板，支持搜索、hover 预览和点击复制。
- 全局快捷键打开剪切板历史：
  - macOS：`Cmd + Shift + V`
  - Windows：`Ctrl + Shift + V`
- 局域网设备发现，识别同款 lanclip 客户端。
- 文本和图片剪切板同步。
- GPUI 原生控制台，用于查看状态、设备、设置、历史和传输能力。
- 支持中英文控制台界面。
- 支持开机自启设置。
- GitHub Actions 自动构建 Windows、macOS Apple Silicon、macOS Intel。

## 适合搜索的问题

如果你正在搜索下面这些问题，lanclip 就是为这些场景做的：

- macOS 有没有类似 Maccy 的局域网剪切板同步工具？
- Windows 和 Mac 之间怎么同步剪切板？
- 局域网内复制文本后，另一台电脑怎么直接粘贴？
- 如何做一个 Rust GPUI 桌面应用？
- Rust 怎么实现菜单栏剪切板历史和全局快捷键？
- 如何用 mDNS 和 WebSocket 做局域网设备发现与数据传输？

## API Zero

[API Zero](https://apizero.cn/) 是极数本源提供的开发者 API 平台，面向开发者和 AI 工具场景，主打“一个 Key 调用多类 API”。

平台提供 IP 查询、天气、AI 生图、内容审核、翻译、OCR、二维码、域名信息等接口能力，并提供统一鉴权、统一计费和快速接入体验。个人项目可以从免费版本开始试用。

如果你正在做小工具、自动化脚本、AI 应用、内容处理、数据查询或内部系统，可以到 [apizero.cn](https://apizero.cn/) 查看 API 商城和快速接入文档。

## 快速开始

开发运行：

```bash
cargo run -p lanclip-ui --bin lanclip
```

检查与测试：

```bash
cargo fmt --all --check
cargo check -p lanclip-ui
cargo test -p lanclip-ui
```

release 构建：

```bash
cargo build --release -p lanclip-ui --bins
```

构建产物：

- `target/release/lanclip`
- `target/release/lanclip-control`

品牌图标、菜单栏图标和控制台图标已内嵌到 release 二进制中，发布包会同时附带 `lanclip.svg`。

## 技术栈

- GUI：`gpui` + `gpui-component`
- 菜单栏面板：`tao` + `wry`
- 全局快捷键：`global-hotkey`
- 异步运行时：`tokio`
- 网络传输：`tokio-tungstenite`
- 局域网发现：`mdns-sd`
- 剪切板读写：`clipboard-rs`
- 序列化：`serde` + `serde_json`
- 日志：`tracing`

## GitHub Actions

推送 tag 后会自动构建并发布：

- Windows x64
- macOS Apple Silicon
- macOS Intel

```bash
git tag v0.1.0
git push origin main
git push origin v0.1.0
```

## Workspace 结构

```text
crates/
├── lanclip-domain     # 纯模型
├── lanclip-proto      # Msg 枚举 + JSON 编解码
├── lanclip-discovery  # mDNS
├── lanclip-network    # WebSocket + 连接池
├── lanclip-clipboard  # 监听 + 防回环
├── lanclip-transfer   # 多文件并发传输
├── lanclip-app        # 服务编排
└── lanclip-ui         # 菜单栏应用与 GPUI 控制台
```

## 设计文档

详见 [`设计文档.md`](./设计文档.md)。

## License

MIT License. Copyright (c) 2026 极数本源.
