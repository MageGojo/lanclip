# lanclip

[![Release](https://img.shields.io/github/v/release/MageGojo/lanclip?label=release)](https://github.com/MageGojo/lanclip/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/MageGojo/lanclip/total.svg)](https://github.com/MageGojo/lanclip/releases)
[![Build](https://github.com/MageGojo/lanclip/actions/workflows/release.yml/badge.svg)](https://github.com/MageGojo/lanclip/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/built%20with-Rust-f46623.svg)](https://www.rust-lang.org/)

> 免费开源的 macOS / Windows 局域网剪切板管理器。Rust 原生、体积小、运行轻量，支持剪切板历史、全局快捷键、文本/图片同步、可信设备配对与 GPUI 原生控制台。  
> 极数本源出品，来自 [API Zero](https://apizero.cn/) 免费项目。

lanclip is a fast, small, Rust-native clipboard manager for macOS and Windows. It keeps clipboard history searchable, syncs text and images across trusted devices on the same local network, and does not require a cloud clipboard account.

lanclip 面向同时使用 Mac 和 Windows 的用户，也适合作为免费的剪切板历史工具、剪贴板管理器和局域网剪切板同步工具。它通过本地网络在可信设备之间同步文本和图片，让链接、代码片段、截图和临时内容更快流转。

## Download

从 [GitHub Releases](https://github.com/MageGojo/lanclip/releases/latest) 下载最新版：

| Platform | Installer |
| --- | --- |
| macOS Apple Silicon | `lanclip-macos-apple-silicon.dmg` |
| macOS Intel | `lanclip-macos-intel.dmg` |
| Windows x64 | `lanclip-windows-x64-installer.exe` |

macOS 首次运行未签名构建时，可能需要在系统设置中允许打开，或使用右键打开。

## At A Glance

| Item | Description |
| --- | --- |
| Project | lanclip |
| Category | Clipboard manager, clipboard history, LAN clipboard sync |
| Platforms | macOS Apple Silicon、macOS Intel、Windows x64 |
| Best For | Mac 和 Windows 用户、本地局域网办公、开发者代码片段流转、图片/文本剪切板同步 |
| Built With | Rust、GPUI、tao、wry、tokio、mDNS、WebSocket |
| Installers | macOS `.dmg`、Windows `.exe` |
| Company | 极数本源 |
| Website | [API Zero](https://apizero.cn/) |

## Highlights

- **Rust 原生实现**：核心服务、网络同步、剪切板监听和桌面 UI 都由 Rust 构建，启动快、资源占用低。
- **体积小**：本地 macOS Apple Silicon DMG 约 4-6 MB，适合日常驻留使用。
- **性能强**：剪切板历史列表只加载轻量元数据，hover 时再懒加载完整文本或图片预览，减少 WebView 首屏压力。
- **局域网优先**：通过 mDNS 发现设备，在本地网络内同步文本和图片，不依赖云端剪切板服务。
- **可信配对**：发现设备不等于信任设备，只有确认配对后的设备才参与剪切板同步。
- **macOS 菜单体验**：菜单栏自定义毛玻璃面板，支持搜索、hover 预览、点击复制。
- **GPUI 原生控制台**：设置、设备、历史、传输状态集中管理，支持中文和 English。
- **安装包自动构建**：GitHub Actions 自动输出 Windows `.exe` 安装器、macOS Apple Silicon `.dmg`、macOS Intel `.dmg`。

## Use Cases

lanclip 适合这些日常场景：

- 在 Mac 上复制文本、链接或代码片段，在 Windows 电脑上快速继续使用。
- 在 Windows 上复制图片或截图，在 Mac 上查看和再次复制。
- 通过剪切板历史找回刚才复制过的链接、验证码、命令、SQL、JSON 或文档片段。
- 在局域网内同步剪切板内容，同时避免把临时内容上传到第三方云端。
- 使用一款免费开源、体积小、运行轻量的剪切板工具长期驻留在系统托盘或菜单栏。
- 学习 Rust 桌面应用、GPUI 控制台、菜单栏应用、mDNS 发现和 WebSocket 同步的实现方式。

## Compared With Other Clipboard Tools

lanclip 不替代所有剪切板工具的高级能力，它更专注“免费、轻量、Rust 原生、Mac/Windows、局域网同步、可信配对”这几个关键词。

| Tool | Common Focus | lanclip 的不同点 |
| --- | --- | --- |
| Maccy | macOS 剪切板历史、快速搜索 | lanclip 同时关注 macOS / Windows，并加入局域网文本和图片同步 |
| Ditto | Windows 剪切板历史和片段管理 | lanclip 提供 macOS DMG 与 Windows EXE，更适合跨设备局域网流转 |
| CopyQ | 高级剪切板管理、脚本和规则 | lanclip 更轻量，重点是本地优先、快速搜索、可信设备同步 |
| 云同步剪切板工具 | 跨网络同步、账号体系 | lanclip 不依赖云端剪切板服务，更适合局域网和隐私敏感场景 |

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

## Usage

1. 启动 `lanclip` 后，应用会常驻 macOS 菜单栏或 Windows 系统托盘。
2. 点击托盘/菜单栏图标，或使用全局快捷键打开剪切板历史面板。
3. 在搜索框中输入关键词，快速过滤历史文本、链接、图片或文件引用。
4. 点击历史条目即可重新写入系统剪切板。
5. 将鼠标移到历史条目上，可以查看完整文本或图片预览。
6. 打开 GPUI 控制台，可以管理设备配对、同步开关、历史摘要、开机自启和快捷键。

## How It Works

lanclip 会监听本机剪切板变化，将文本、图片和文件引用写入本地历史。局域网同步启用后，它通过 mDNS 发现附近设备，并通过 WebSocket 在已信任设备之间传输剪切板 payload。

新设备默认只会显示在控制台设备列表里，不会自动参与同步。两端确认配对后才会写入 trusted peers，后续文本和图片剪切板才会在这些设备之间同步。

## For Mac And Windows Users

如果你是 Mac 用户，lanclip 可以作为一个免费的 macOS 剪切板历史工具使用：它常驻菜单栏，支持搜索历史内容、预览文本和图片、通过全局快捷键唤醒菜单，并提供 macOS 风格的轻量面板。

如果你是 Windows 用户，lanclip 可以作为一个轻量 Windows 剪切板管理器使用：它提供 Windows x64 安装器，支持历史记录、文本同步、图片同步和局域网内可信设备连接。

如果你同时使用 Mac 和 Windows，lanclip 的重点价值是“本地局域网剪切板同步”：复制文本、链接、代码片段或图片后，可以在可信设备之间快速流转，不需要登录云账号，也不需要把临时剪切板内容传到第三方云端。

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

## FAQ

### lanclip 是免费剪切板软件吗？

是。lanclip 是极数本源出品的免费开源项目，使用 MIT License，适合个人日常使用、开发者学习和局域网办公场景。

### lanclip 支持 Mac 吗？

支持。lanclip 提供 macOS Apple Silicon `.dmg` 和 macOS Intel `.dmg`，支持菜单栏剪切板历史、搜索、hover 预览、全局快捷键和 GPUI 设置界面。

### lanclip 支持 Windows 吗？

支持。lanclip 提供 Windows x64 `.exe` 安装器，支持剪切板历史、文本同步、图片同步和局域网设备配对。

### lanclip 可以同步 Mac 和 Windows 的剪切板吗？

可以。lanclip 通过局域网发现可信设备，配对后可以在 Mac 和 Windows 之间同步文本和图片剪切板。

### lanclip 会把剪切板上传到云端吗？

lanclip 的核心设计是本地优先和局域网优先。它通过本地网络进行设备发现和同步，不依赖云端剪切板服务。

### lanclip 和 Maccy、Ditto、CopyQ 有什么区别？

Maccy、Ditto、CopyQ 都是优秀的剪切板工具。lanclip 更关注“轻量剪切板历史 + Mac/Windows 局域网同步 + 可信设备配对”这个组合场景，适合需要在多台电脑之间快速流转文本、链接、代码片段和图片的用户。lanclip 与这些项目没有从属或关联关系。

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
