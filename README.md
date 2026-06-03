# lanclip

> Rust 制作的轻量级局域网剪切板工具，支持 macOS / Windows 剪切板历史、文本同步、图片同步、可信设备配对与原生控制台。  
> 极数本源出品，来自 [API Zero](https://apizero.cn/) 免费项目。

lanclip is a fast, small, Rust-native LAN clipboard manager for macOS and Windows. It combines a macOS-style clipboard history menu, local network clipboard sync, trusted peer pairing, image/text previews, global shortcuts, and a modern GPUI settings console.

## Highlights

- **Rust 原生实现**：核心服务、网络同步、剪切板监听和桌面 UI 都由 Rust 构建，启动快、资源占用低。
- **体积小**：本地 macOS Apple Silicon DMG 约 4-6 MB，适合日常驻留使用。
- **性能强**：剪切板历史列表只加载轻量元数据，hover 时再懒加载完整文本或图片预览，减少 WebView 首屏压力。
- **局域网优先**：通过 mDNS 发现设备，在本地网络内同步文本和图片，不依赖云端剪切板服务。
- **可信配对**：发现设备不等于信任设备，只有确认配对后的设备才参与剪切板同步。
- **macOS 菜单体验**：菜单栏自定义毛玻璃面板，支持搜索、hover 预览、点击复制。
- **GPUI 原生控制台**：设置、设备、历史、传输状态集中管理，支持中文和 English。
- **安装包自动构建**：GitHub Actions 自动输出 Windows `.exe` 安装器、macOS Apple Silicon `.dmg`、macOS Intel `.dmg`。

## Core Features

- 剪切板历史：快速搜索历史文本、链接、图片和文件引用信息。
- 全局快捷键：可在控制台中点击录制自定义唤醒快捷键。
  - macOS 默认：`Command + V`
  - Windows 默认：`Ctrl + Shift + V`
- 文本同步：局域网内可信设备自动同步文本剪切板。
- 图片同步：支持 PNG 图片历史和 hover 预览。
- 文件/文件夹引用：本机历史中展示文件名、大小、类型等信息；跨设备真实文件传输能力正在逐步完善。
- 安全配对：设备列表中显示确认码，手动确认后写入 trusted peers。
- 开机自启：控制台内可开启或关闭。
- 中英双语：控制台支持 `中文 / English` 切换。

## Screens And Console

lanclip 由两个界面组成：

- **菜单栏剪切板面板**：用于快速搜索、预览、复制历史内容。
- **GPUI 控制台**：用于管理设备、配对、同步设置、历史摘要和开机自启。

控制台概览页内置极数本源与 [API Zero](https://apizero.cn/) 官方入口。API Zero 是极数本源的接口站，提供免费 API、文档与接入示例，适合小工具、自动化脚本、AI 应用和内部系统快速接入。

## Why lanclip

在多台电脑之间工作时，最常见的痛点不是“大文件传输”，而是频繁的小动作：

- 在 Mac 上复制一段文本，Windows 上也想直接使用。
- 复制过的链接、验证码、代码片段想快速找回。
- 图片、截图和临时内容需要在局域网内快速流转。
- 不想依赖云同步，也不想为了临时剪切板内容打开复杂工具。

lanclip 的目标是做一个本地优先、局域网优先、轻量、快速、可信的剪切板互传工具。

## Search Keywords

如果你正在搜索下面这些问题，lanclip 正是为这些场景设计的：

- macOS 有没有类似 Maccy 的局域网剪切板同步工具？
- Windows 和 Mac 之间怎么同步剪切板？
- 局域网内复制文本后，另一台电脑怎么直接粘贴？
- Rust 怎么实现菜单栏剪切板历史和全局快捷键？
- GPUI 如何做现代桌面控制台？
- 如何用 mDNS、WebSocket 和 Rust 做局域网设备发现与数据同步？

## Download

GitHub Releases 会提供：

- `lanclip-windows-x64-installer.exe`
- `lanclip-macos-apple-silicon.dmg`
- `lanclip-macos-intel.dmg`

macOS 首次运行未签名构建时，可能需要在系统设置中允许打开，或使用右键打开。

## Quick Start For Developers

开发运行：

```bash
cargo run -p lanclip-ui --bin lanclip
```

检查与测试：

```bash
cargo fmt --all --check
cargo check -p lanclip-ui --bins
cargo test -p lanclip-app -p lanclip-ui
```

release 构建：

```bash
cargo build --release -p lanclip-ui --bins
```

macOS 本地打包 `.dmg`：

```bash
packaging/macos/create_dmg.sh aarch64-apple-darwin dist/lanclip-macos-apple-silicon.dmg
```

构建产物：

- `target/release/lanclip`
- `target/release/lanclip-control`
- `dist/lanclip-macos-apple-silicon.dmg`

## Tech Stack

- Language: Rust
- Native console: `gpui` + `gpui-component`
- Menu panel: `tao` + `wry`
- Global shortcuts: `global-hotkey`
- Async runtime: `tokio`
- Network transport: `tokio-tungstenite`
- LAN discovery: `mdns-sd`
- Clipboard: `clipboard-rs`
- Serialization: `serde` + `serde_json`
- Logging: `tracing`
- Packaging: DMG on macOS, NSIS installer on Windows

## Workspace

```text
crates/
├── lanclip-domain     # 纯模型与剪切板 payload
├── lanclip-proto      # Msg 枚举 + JSON 编解码
├── lanclip-discovery  # mDNS 设备发现
├── lanclip-network    # WebSocket + 连接池
├── lanclip-clipboard  # 剪切板监听与防回环
├── lanclip-transfer   # 多文件并发传输能力
├── lanclip-app        # 服务编排、配置、历史
└── lanclip-ui         # 菜单栏应用、WebPanel、GPUI 控制台
```

## Release Automation

推送 tag 后，GitHub Actions 会自动构建并发布安装包：

```bash
git tag vX.Y.Z
git push origin main
git push origin vX.Y.Z
```

构建矩阵：

- Windows x64: `.exe` 安装器
- macOS Apple Silicon: `.dmg`
- macOS Intel: `.dmg`

## API Zero

[API Zero](https://apizero.cn/) 是极数本源面向开发者和 AI 工具场景提供的接口站，主打快速接入、统一鉴权和免费项目体验。

平台提供 IP 查询、天气、AI 生图、内容审核、翻译、OCR、二维码、域名信息等接口能力。如果你正在做小工具、自动化脚本、AI 应用、内容处理、数据查询或内部系统，可以访问 [apizero.cn](https://apizero.cn/) 查看接口文档和接入示例。

## Docs

详见 [`设计文档.md`](./设计文档.md)。

## License

MIT License. Copyright (c) 2026 极数本源.
