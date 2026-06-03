#![recursion_limit = "1024"]

mod control_api;

use std::borrow::Cow;

use control_api::client;
use control_api::{ControlPeerDto, ControlStateDto, HistoryItemDto, SettingsPatchDto};
use gpui::prelude::*;
use gpui::*;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, Root};

struct ControlAssets {
    icons: &'static [(&'static str, &'static [u8])],
}

impl AssetSource for ControlAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(self
            .icons
            .iter()
            .find_map(|(asset_path, bytes)| (*asset_path == path).then_some(Cow::Borrowed(*bytes))))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        if path == "icons" {
            Ok(self
                .icons
                .iter()
                .filter_map(|(asset_path, _)| asset_path.strip_prefix("icons/"))
                .map(SharedString::from)
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}

fn control_assets() -> ControlAssets {
    ControlAssets {
        icons: &[
            (
                "icons/book-open.svg",
                include_bytes!("../assets/icons/book-open.svg"),
            ),
            (
                "icons/case-sensitive.svg",
                include_bytes!("../assets/icons/case-sensitive.svg"),
            ),
            ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
            ("icons/file.svg", include_bytes!("../assets/icons/file.svg")),
            (
                "icons/gallery-vertical-end.svg",
                include_bytes!("../assets/icons/gallery-vertical-end.svg"),
            ),
            (
                "icons/globe.svg",
                include_bytes!("../assets/icons/globe.svg"),
            ),
            (
                "icons/lanclip.svg",
                include_bytes!("../assets/icons/lanclip.svg"),
            ),
            (
                "icons/layout-dashboard.svg",
                include_bytes!("../assets/icons/layout-dashboard.svg"),
            ),
            (
                "icons/panel-left.svg",
                include_bytes!("../assets/icons/panel-left.svg"),
            ),
            (
                "icons/replace.svg",
                include_bytes!("../assets/icons/replace.svg"),
            ),
            (
                "icons/settings-2.svg",
                include_bytes!("../assets/icons/settings-2.svg"),
            ),
        ],
    }
}

fn tone_sidebar_border() -> Rgba {
    rgba(0xbfd5eb80)
}

fn box_shadow(x: Pixels, y: Pixels, blur: Pixels, spread: Pixels, color: Hsla) -> BoxShadow {
    BoxShadow {
        offset: point(x, y),
        blur_radius: blur,
        spread_radius: spread,
        color,
    }
}

fn tone_panel_border() -> Rgba {
    rgba(0xd7e4f3b8)
}

fn tone_divider() -> Rgba {
    rgba(0xdce8f4a8)
}

fn tone_text() -> Rgba {
    rgba(0x111827ff)
}

fn tone_muted() -> Rgba {
    rgba(0x66758aff)
}

fn tone_accent_dark() -> Rgba {
    rgba(0x075eb5ff)
}

fn root_bg() -> Background {
    linear_gradient(
        135.0,
        linear_color_stop(rgba(0xf8fbffff), 0.0),
        linear_color_stop(rgba(0xe9f4fff0), 1.0),
    )
}

fn sidebar_bg() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgba(0xdff2ffcf), 0.0),
        linear_color_stop(rgba(0xf9fcffc2), 1.0),
    )
}

fn content_bg() -> Background {
    linear_gradient(
        160.0,
        linear_color_stop(rgba(0xffffffd8), 0.0),
        linear_color_stop(rgba(0xf1f7fee8), 1.0),
    )
}

fn glass_bg() -> Background {
    linear_gradient(
        165.0,
        linear_color_stop(rgba(0xfffffff7), 0.0),
        linear_color_stop(rgba(0xf8fbffe6), 1.0),
    )
}

fn glass_bg_subtle() -> Background {
    linear_gradient(
        165.0,
        linear_color_stop(rgba(0xfffffff0), 0.0),
        linear_color_stop(rgba(0xf0f7ffd8), 1.0),
    )
}

fn accent_bg() -> Background {
    linear_gradient(
        135.0,
        linear_color_stop(rgba(0x0a84ffff), 0.0),
        linear_color_stop(rgba(0x006fe8ff), 1.0),
    )
}

fn active_nav_bg() -> Background {
    linear_gradient(
        135.0,
        linear_color_stop(rgba(0xfffffff2), 0.0),
        linear_color_stop(rgba(0xddeeffeb), 1.0),
    )
}

fn hover_nav_bg() -> Background {
    linear_gradient(
        135.0,
        linear_color_stop(rgba(0xffffffe8), 0.0),
        linear_color_stop(rgba(0xe8f4ffe0), 1.0),
    )
}

fn panel_shadow() -> Vec<BoxShadow> {
    vec![
        box_shadow(
            px(0.0),
            px(18.0),
            px(32.0),
            px(-22.0),
            hsla(212.0, 0.48, 0.28, 0.18),
        ),
        box_shadow(
            px(0.0),
            px(2.0),
            px(8.0),
            px(-4.0),
            hsla(215.0, 0.34, 0.22, 0.16),
        ),
    ]
}

fn soft_shadow() -> Vec<BoxShadow> {
    vec![
        box_shadow(
            px(0.0),
            px(14.0),
            px(24.0),
            px(-18.0),
            hsla(210.0, 0.42, 0.24, 0.14),
        ),
        box_shadow(
            px(0.0),
            px(1.0),
            px(3.0),
            px(0.0),
            hsla(210.0, 0.22, 0.2, 0.08),
        ),
    ]
}

fn inset_highlight() -> Vec<BoxShadow> {
    vec![
        box_shadow(
            px(0.0),
            px(1.0),
            px(0.0),
            px(0.0),
            hsla(0.0, 0.0, 1.0, 0.78),
        ),
        box_shadow(
            px(0.0),
            px(12.0),
            px(24.0),
            px(-18.0),
            hsla(210.0, 0.4, 0.28, 0.12),
        ),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Devices,
    Settings,
    History,
    Transfers,
}

struct ControlApp {
    base_url: String,
    token: String,
    state: ControlStateDto,
    tab: Tab,
    error: String,
}

impl ControlApp {
    fn new(base_url: String, token: String) -> Self {
        let mut app = Self {
            base_url,
            token,
            state: ControlStateDto::default(),
            tab: Tab::Overview,
            error: String::new(),
        };
        app.refresh_now();
        app
    }

    fn refresh_now(&mut self) {
        match client::get_state(&self.base_url, &self.token) {
            Ok(state) => {
                self.state = state;
                self.error.clear();
            }
            Err(e) => self.error = e.to_string(),
        }
    }

    fn tr(&self, key: &'static str) -> &'static str {
        let zh = self.state.language != "en";
        match (zh, key) {
            (true, "overview") => "概览",
            (false, "overview") => "Overview",
            (true, "devices") => "设备",
            (false, "devices") => "Devices",
            (true, "settings") => "设置",
            (false, "settings") => "Settings",
            (true, "history") => "历史",
            (false, "history") => "History",
            (true, "transfers") => "传输",
            (false, "transfers") => "Transfers",
            (true, "sync") => "同步",
            (false, "sync") => "Sync",
            (true, "trusted") => "已信任",
            (false, "trusted") => "Trusted",
            (true, "untrusted") => "未配对",
            (false, "untrusted") => "Not paired",
            (true, "connected") => "在线",
            (false, "connected") => "Connected",
            (true, "offline") => "离线",
            (false, "offline") => "Offline",
            (true, "confirm") => "确认配对",
            (false, "confirm") => "Confirm",
            (true, "forget") => "取消信任",
            (false, "forget") => "Forget",
            (true, "clips") => "历史记录",
            (false, "clips") => "Clips",
            (true, "peers") => "在线设备",
            (false, "peers") => "Peers",
            (true, "port") => "端口",
            (false, "port") => "Port",
            (true, "device") => "本机设备",
            (false, "device") => "This Device",
            (true, "language") => "语言",
            (false, "language") => "Language",
            (true, "enabled") => "已开启",
            (false, "enabled") => "Enabled",
            (true, "disabled") => "已关闭",
            (false, "disabled") => "Disabled",
            (true, "text") => "同步文本",
            (false, "text") => "Sync Text",
            (true, "images") => "同步图片",
            (false, "images") => "Sync Images",
            (true, "files") => "显示文件引用",
            (false, "files") => "Show File References",
            (true, "launch_at_login") => "开机自启",
            (false, "launch_at_login") => "Launch at Login",
            (true, "refresh") => "刷新",
            (false, "refresh") => "Refresh",
            (true, "device_id") => "设备 ID",
            (false, "device_id") => "Device ID",
            (true, "local_status") => "本机状态",
            (false, "local_status") => "Local Status",
            (true, "pairing") => "配对码",
            (false, "pairing") => "Pairing Code",
            (true, "recent") => "最近活动",
            (false, "recent") => "Recent Activity",
            (true, "waiting_devices") => "等待附近设备",
            (false, "waiting_devices") => "Waiting for nearby devices",
            (true, "waiting_history") => "复制内容后会显示在这里。",
            (false, "waiting_history") => "Clipboard items will appear here after you copy.",
            (true, "on") => "开启",
            (false, "on") => "On",
            (true, "off") => "关闭",
            (false, "off") => "Off",
            (true, "empty_devices") => "还没有连接到局域网设备。",
            (false, "empty_devices") => "No LAN peers are connected yet.",
            (true, "empty_history") => "还没有剪切板历史。",
            (false, "empty_history") => "No clipboard history yet.",
            (true, "transfer_note") => "文件传输能力已保留，本轮先展示状态。",
            (false, "transfer_note") => {
                "File transfer support is present; this view shows status for now."
            }
            (true, "transfer_detail") => "文本和图片同步可用；文件/文件夹本轮仍按引用信息展示，等可信传输开启后再做真实文件传输。",
            (false, "transfer_detail") => {
                "Text and image sync are available. Files and folders stay display-only until trusted transfer is enabled."
            }
            _ => key,
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn save_settings(&mut self, patch: SettingsPatchDto, cx: &mut Context<Self>) {
        match client::update_settings(&self.base_url, &self.token, &patch) {
            Ok(state) => {
                self.state = state;
                self.error.clear();
            }
            Err(e) => self.error = e.to_string(),
        }
        cx.notify();
    }

    fn confirm_peer(&mut self, peer_id: String, cx: &mut Context<Self>) {
        match client::confirm_peer(&self.base_url, &self.token, &peer_id) {
            Ok(state) => {
                self.state = state;
                self.error.clear();
            }
            Err(e) => self.error = e.to_string(),
        }
        cx.notify();
    }

    fn cancel_peer(&mut self, peer_id: String, cx: &mut Context<Self>) {
        match client::cancel_peer(&self.base_url, &self.token, &peer_id) {
            Ok(state) => {
                self.state = state;
                self.error.clear();
            }
            Err(e) => self.error = e.to_string(),
        }
        cx.notify();
    }
}

impl EventEmitter<()> for ControlApp {}

impl Render for ControlApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .overflow_hidden()
            .bg(root_bg())
            .text_color(tone_text())
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .p_6()
                    .bg(content_bg())
                    .border_l_1()
                    .border_color(rgba(0xffffff80))
                    .child(self.header(cx))
                    .child(match self.tab {
                        Tab::Overview => self.overview(cx).into_any_element(),
                        Tab::Devices => self.devices(cx).into_any_element(),
                        Tab::Settings => self.settings(cx).into_any_element(),
                        Tab::History => self.history(cx).into_any_element(),
                        Tab::Transfers => self.transfers(cx).into_any_element(),
                    })
                    .when(!self.error.is_empty(), |p| {
                        p.child(
                            div()
                                .mt_4()
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgba(0xffc7c7cc))
                                .bg(rgba(0xfff0f0f0))
                                .text_sm()
                                .text_color(rgba(0x9f1d1dff))
                                .child(self.error.clone()),
                        )
                    }),
            )
    }
}

impl ControlApp {
    fn sidebar(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .w(px(238.0))
            .h_full()
            .p_5()
            .pt_6()
            .flex()
            .flex_col()
            .justify_between()
            .bg(sidebar_bg())
            .border_r_1()
            .border_color(tone_sidebar_border())
            .child(
                div()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .mb_6()
                            .child(app_mark())
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(tone_text())
                                            .truncate()
                                            .child("lanclip"),
                                    )
                                    .child(
                                        div().text_xs().text_color(tone_muted()).truncate().child(
                                            format!(
                                                "{} · {}",
                                                self.state.device_name, self.state.short_device_id
                                            ),
                                        ),
                                    ),
                            ),
                    )
                    .child(nav_item(
                        "overview",
                        IconName::LayoutDashboard,
                        self.tr("overview"),
                        self.tab == Tab::Overview,
                        cx,
                        |this, cx| this.set_tab(Tab::Overview, cx),
                    ))
                    .child(nav_item(
                        "devices",
                        IconName::PanelLeft,
                        self.tr("devices"),
                        self.tab == Tab::Devices,
                        cx,
                        |this, cx| this.set_tab(Tab::Devices, cx),
                    ))
                    .child(nav_item(
                        "settings",
                        IconName::Settings2,
                        self.tr("settings"),
                        self.tab == Tab::Settings,
                        cx,
                        |this, cx| this.set_tab(Tab::Settings, cx),
                    ))
                    .child(nav_item(
                        "history",
                        IconName::BookOpen,
                        self.tr("history"),
                        self.tab == Tab::History,
                        cx,
                        |this, cx| this.set_tab(Tab::History, cx),
                    ))
                    .child(nav_item(
                        "transfers",
                        IconName::Replace,
                        self.tr("transfers"),
                        self.tab == Tab::Transfers,
                        cx,
                        |this, cx| this.set_tab(Tab::Transfers, cx),
                    )),
            )
            .child(
                div()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0xffffffb8))
                    .bg(glass_bg_subtle())
                    .p_3()
                    .shadow(soft_shadow())
                    .child(
                        div()
                            .text_xs()
                            .text_color(tone_muted())
                            .child(self.tr("local_status").to_string()),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tone_text())
                            .truncate()
                            .child(self.header_detail()),
                    ),
            )
    }

    fn header(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .mb_6()
            .flex_none()
            .min_w(px(0.0))
            .child(
                div()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_3xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(tone_text())
                            .truncate()
                            .child(self.tab_title().to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(tone_muted())
                            .truncate()
                            .child(self.header_detail()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap_2()
                    .child(small_button(
                        "lang_zh",
                        "中文",
                        self.state.language != "en",
                        cx,
                        |this, cx| {
                            this.save_settings(
                                SettingsPatchDto {
                                    language: Some("zh".into()),
                                    ..Default::default()
                                },
                                cx,
                            )
                        },
                    ))
                    .child(small_button(
                        "lang_en",
                        "English",
                        self.state.language == "en",
                        cx,
                        |this, cx| {
                            this.save_settings(
                                SettingsPatchDto {
                                    language: Some("en".into()),
                                    ..Default::default()
                                },
                                cx,
                            )
                        },
                    ))
                    .child(small_button(
                        "refresh",
                        self.tr("refresh"),
                        false,
                        cx,
                        |this, cx| {
                            this.refresh_now();
                            cx.notify();
                        },
                    )),
            )
    }

    fn tab_title(&self) -> &'static str {
        match self.tab {
            Tab::Overview => self.tr("overview"),
            Tab::Devices => self.tr("devices"),
            Tab::Settings => self.tr("settings"),
            Tab::History => self.tr("history"),
            Tab::Transfers => self.tr("transfers"),
        }
    }

    fn header_detail(&self) -> String {
        if self.state.clipboard_sync_enabled {
            format!(
                "{} · {} {}",
                self.tr("sync"),
                self.tr("enabled"),
                self.state.port
            )
        } else {
            format!("{} · {}", self.tr("sync"), self.tr("disabled"))
        }
    }

    fn overview(&mut self, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .min_w(px(0.0))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .mb_5()
                    .child(metric_card(
                        IconName::Globe,
                        self.tr("port"),
                        self.state.port.to_string(),
                    ))
                    .child(metric_card(
                        IconName::PanelLeft,
                        self.tr("peers"),
                        self.state.connected_count.to_string(),
                    ))
                    .child(metric_card(
                        IconName::BookOpen,
                        self.tr("clips"),
                        self.state.history_count.to_string(),
                    )),
            )
            .child(
                glass_panel()
                    .child(section_label(self.tr("device")))
                    .child(row_text(self.tr("device"), &self.state.device_name))
                    .child(row_text(self.tr("device_id"), &self.state.device_id))
                    .child(row_text(
                        self.tr("sync"),
                        if self.state.clipboard_sync_enabled {
                            self.tr("enabled")
                        } else {
                            self.tr("disabled")
                        },
                    )),
            )
    }

    fn devices(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let peers = self.state.peers.clone();
        glass_panel()
            .when(peers.is_empty(), |p| {
                p.child(empty_state(
                    IconName::PanelLeft,
                    self.tr("waiting_devices"),
                    self.tr("empty_devices"),
                ))
            })
            .children(
                peers
                    .into_iter()
                    .map(|peer| self.peer_row(peer, cx).into_any_element()),
            )
    }

    fn peer_row(&mut self, peer: ControlPeerDto, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let status = if peer.connected {
            self.tr("connected")
        } else {
            self.tr("offline")
        };
        let trust = if peer.trusted {
            self.tr("trusted")
        } else {
            self.tr("untrusted")
        };
        let peer_confirm = peer.id.clone();
        let peer_cancel = peer.id.clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .min_w(px(0.0))
            .py_3()
            .border_b_1()
            .border_color(tone_divider())
            .child(
                div()
                    .flex()
                    .min_w(px(0.0))
                    .items_center()
                    .gap_3()
                    .child(icon_tile(IconName::PanelLeft, false))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tone_text())
                                    .truncate()
                                    .child(peer.short_id.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(tone_muted())
                                    .truncate()
                                    .child(format!("{status} · {trust} · {}", peer.code)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap_2()
                    .child(small_button(
                        "confirm_peer",
                        self.tr("confirm"),
                        peer.trusted,
                        cx,
                        move |this, cx| this.confirm_peer(peer_confirm.clone(), cx),
                    ))
                    .child(small_button(
                        "cancel_peer",
                        self.tr("forget"),
                        false,
                        cx,
                        move |this, cx| this.cancel_peer(peer_cancel.clone(), cx),
                    )),
            )
    }

    fn settings(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let sync = self.state.clipboard_sync_enabled;
        let text = self.state.sync_text;
        let images = self.state.sync_images;
        let files = self.state.show_file_refs;
        let launch = self.state.launch_at_login;
        glass_panel()
            .child(section_label(self.tr("settings")))
            .child(setting_toggle(
                "launch_at_login",
                self.tr("launch_at_login"),
                launch,
                self.tr("on"),
                self.tr("off"),
                cx,
                |this, cx| {
                    this.save_settings(
                        SettingsPatchDto {
                            launch_at_login: Some(!this.state.launch_at_login),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(setting_toggle(
                "sync",
                self.tr("sync"),
                sync,
                self.tr("on"),
                self.tr("off"),
                cx,
                |this, cx| {
                    this.save_settings(
                        SettingsPatchDto {
                            clipboard_sync_enabled: Some(!this.state.clipboard_sync_enabled),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(setting_toggle(
                "text",
                self.tr("text"),
                text,
                self.tr("on"),
                self.tr("off"),
                cx,
                |this, cx| {
                    this.save_settings(
                        SettingsPatchDto {
                            sync_text: Some(!this.state.sync_text),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(setting_toggle(
                "images",
                self.tr("images"),
                images,
                self.tr("on"),
                self.tr("off"),
                cx,
                |this, cx| {
                    this.save_settings(
                        SettingsPatchDto {
                            sync_images: Some(!this.state.sync_images),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(setting_toggle(
                "files",
                self.tr("files"),
                files,
                self.tr("on"),
                self.tr("off"),
                cx,
                |this, cx| {
                    this.save_settings(
                        SettingsPatchDto {
                            show_file_refs: Some(!this.state.show_file_refs),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(row_text(
                self.tr("language"),
                if self.state.language == "en" {
                    "English"
                } else {
                    "中文"
                },
            ))
    }

    fn history(&mut self, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        let items = self.state.history.clone();
        glass_panel()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(items.is_empty(), |p| {
                p.child(empty_state(
                    IconName::BookOpen,
                    self.tr("empty_history"),
                    self.tr("waiting_history"),
                ))
            })
            .when(!items.is_empty(), |p| {
                p.child(
                    div()
                        .id("history-scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .pr_1()
                        .children(
                            items
                                .into_iter()
                                .map(history_row)
                                .map(IntoElement::into_any_element),
                        )
                        .overflow_y_scrollbar(),
                )
            })
    }

    fn transfers(&mut self, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        glass_panel()
            .child(section_label(self.tr("transfers")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .mb_2()
                    .child(icon_tile(IconName::Replace, false))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("transfers")),
                    ),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_sm()
                    .text_color(tone_text())
                    .child(self.tr("transfer_note")),
            )
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(tone_muted())
                    .child(self.tr("transfer_detail")),
            )
    }
}

fn nav_item(
    id: &str,
    icon: IconName,
    label: &str,
    active: bool,
    cx: &mut Context<'_, ControlApp>,
    f: impl Fn(&mut ControlApp, &mut Context<ControlApp>) + 'static,
) -> impl IntoElement {
    div()
        .id(ElementId::from(SharedString::from(id.to_string())))
        .flex()
        .items_center()
        .gap_2()
        .min_w(px(0.0))
        .mb_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(if active {
            active_nav_bg()
        } else {
            linear_gradient(
                135.0,
                linear_color_stop(rgba(0xffffff00), 0.0),
                linear_color_stop(rgba(0xffffff00), 1.0),
            )
        })
        .when(active, |s| {
            s.border_1()
                .border_color(rgba(0xffffffc8))
                .shadow(vec![box_shadow(
                    px(0.0),
                    px(10.0),
                    px(20.0),
                    px(-16.0),
                    hsla(210.0, 0.45, 0.34, 0.18),
                )])
        })
        .text_color(if active {
            tone_accent_dark()
        } else {
            rgba(0x334155ff)
        })
        .cursor(CursorStyle::PointingHand)
        .hover(|s| s.bg(hover_nav_bg()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| f(this, cx)))
        .child(icon_tile(icon, active))
        .child(
            div()
                .min_w(px(0.0))
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(label.to_string()),
        )
}

fn app_mark() -> impl IntoElement {
    div()
        .w(px(38.0))
        .h(px(38.0))
        .rounded(px(11.0))
        .border_1()
        .border_color(rgba(0x80c7ffff))
        .bg(accent_bg())
        .shadow(vec![
            box_shadow(
                px(0.0),
                px(14.0),
                px(24.0),
                px(-14.0),
                hsla(207.0, 0.78, 0.42, 0.38),
            ),
            box_shadow(
                px(0.0),
                px(1.0),
                px(0.0),
                px(0.0),
                hsla(0.0, 0.0, 1.0, 0.42),
            ),
        ])
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path("icons/lanclip.svg")
                .size_5()
                .text_color(rgba(0xffffffff)),
        )
}

fn icon_tile(icon: IconName, active: bool) -> impl IntoElement {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex_none()
        .rounded_md()
        .border_1()
        .border_color(if active {
            rgba(0x86c5ffff)
        } else {
            rgba(0xd6e5f4c8)
        })
        .bg(if active {
            rgba(0xfffffffa)
        } else {
            rgba(0xffffffe0)
        })
        .shadow(vec![box_shadow(
            px(0.0),
            px(8.0),
            px(16.0),
            px(-14.0),
            hsla(210.0, 0.32, 0.28, 0.16),
        )])
        .flex()
        .items_center()
        .justify_center()
        .text_color(if active {
            tone_accent_dark()
        } else {
            tone_muted()
        })
        .child(Icon::new(icon).size_4().text_color(if active {
            tone_accent_dark()
        } else {
            tone_muted()
        }))
}

fn glass_panel() -> Div {
    div()
        .rounded(px(14.0))
        .border_1()
        .border_color(tone_panel_border())
        .bg(glass_bg())
        .p_5()
        .min_w(px(0.0))
        .shadow(panel_shadow())
}

fn metric_card(icon: IconName, label: &str, value: String) -> impl IntoElement {
    div()
        .w(px(174.0))
        .min_w(px(0.0))
        .rounded(px(14.0))
        .border_1()
        .border_color(tone_panel_border())
        .bg(glass_bg())
        .p_4()
        .shadow(inset_highlight())
        .hover(|s| s.bg(glass_bg_subtle()).shadow(panel_shadow()))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(icon_tile(icon, false))
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_xs()
                        .text_color(tone_muted())
                        .truncate()
                        .child(label.to_string()),
                ),
        )
        .child(
            div()
                .mt_3()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .text_color(tone_text())
                .truncate()
                .child(value),
        )
}

fn section_label(label: &str) -> impl IntoElement {
    div()
        .mb_3()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tone_muted())
        .truncate()
        .child(label.to_string())
}

fn row_text(label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .gap_4()
        .min_w(px(0.0))
        .py_3()
        .border_b_1()
        .border_color(tone_divider())
        .child(
            div()
                .flex_none()
                .text_sm()
                .text_color(tone_muted())
                .truncate()
                .child(label.to_string()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .text_right()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tone_text())
                .truncate()
                .child(value.to_string()),
        )
}

fn setting_toggle(
    id: &str,
    label: &str,
    value: bool,
    on_label: &str,
    off_label: &str,
    cx: &mut Context<'_, ControlApp>,
    f: impl Fn(&mut ControlApp, &mut Context<ControlApp>) + 'static,
) -> impl IntoElement {
    div()
        .id(ElementId::from(SharedString::from(id.to_string())))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .min_w(px(0.0))
        .py_3()
        .border_b_1()
        .border_color(tone_divider())
        .cursor(CursorStyle::PointingHand)
        .hover(|s| s.bg(rgba(0xf5f9ff9c)))
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| f(this, cx)))
        .child(
            div()
                .min_w(px(0.0))
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tone_text())
                .truncate()
                .child(label.to_string()),
        )
        .child(
            div()
                .id(ElementId::from(SharedString::from(id.to_string())))
                .flex()
                .flex_none()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if value {
                            tone_accent_dark()
                        } else {
                            tone_muted()
                        })
                        .truncate()
                        .child(if value { on_label } else { off_label }.to_string()),
                )
                .child(toggle_switch(value)),
        )
}

fn toggle_switch(value: bool) -> impl IntoElement {
    div()
        .w(px(42.0))
        .h(px(24.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(if value {
            rgba(0x66b8ffdd)
        } else {
            rgba(0xd5e3f2e8)
        })
        .bg(if value {
            rgba(0x0a84ffef)
        } else {
            rgba(0xf8fbfff0)
        })
        .shadow(vec![box_shadow(
            px(0.0),
            px(8.0),
            px(16.0),
            px(-14.0),
            hsla(210.0, 0.36, 0.28, 0.16),
        )])
        .flex()
        .items_center()
        .justify_start()
        .p(px(2.0))
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded(px(999.0))
                .bg(rgba(0xffffffff))
                .shadow(vec![box_shadow(
                    px(0.0),
                    px(4.0),
                    px(10.0),
                    px(-4.0),
                    hsla(210.0, 0.28, 0.24, 0.2),
                )])
                .when(value, |this| this.ml(px(18.0))),
        )
}

fn small_button(
    id: &str,
    label: &str,
    active: bool,
    cx: &mut Context<'_, ControlApp>,
    f: impl Fn(&mut ControlApp, &mut Context<ControlApp>) + 'static,
) -> impl IntoElement {
    div()
        .id(ElementId::from(SharedString::from(id.to_string())))
        .px_3()
        .py_1()
        .rounded(px(8.0))
        .border_1()
        .border_color(if active {
            rgba(0x72bdffff)
        } else {
            rgba(0xd8e4f0e8)
        })
        .bg(if active {
            accent_bg()
        } else {
            glass_bg_subtle()
        })
        .text_color(if active {
            rgba(0xffffffff)
        } else {
            rgba(0x334155ff)
        })
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .shadow(vec![box_shadow(
            px(0.0),
            px(8.0),
            px(18.0),
            px(-16.0),
            hsla(210.0, 0.35, 0.28, 0.16),
        )])
        .cursor(CursorStyle::PointingHand)
        .hover(|s| s.opacity(0.92))
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| f(this, cx)))
        .child(label.to_string())
}

fn history_row(item: HistoryItemDto) -> impl IntoElement {
    let icon = history_icon(&item.kind);
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .min_w(px(0.0))
        .px_2()
        .py_3()
        .rounded_lg()
        .border_b_1()
        .border_color(tone_divider())
        .hover(|s| s.bg(rgba(0xf5f9ffb8)))
        .child(
            div()
                .flex()
                .min_w(px(0.0))
                .items_center()
                .gap_3()
                .child(icon_tile(icon, false))
                .child(
                    div()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_sm()
                                .text_color(tone_text())
                                .truncate()
                                .child(item.title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(tone_muted())
                                .truncate()
                                .child(format!(
                                    "{} · {} · {}",
                                    item.detail, item.source, item.time
                                )),
                        ),
                ),
        )
        .child(
            div()
                .flex_none()
                .px_2()
                .py_1()
                .rounded(px(8.0))
                .border_1()
                .border_color(rgba(0xc9e3ffcc))
                .bg(rgba(0xeaf5ffe8))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tone_accent_dark())
                .child(item.kind),
        )
}

fn history_icon(kind: &str) -> IconName {
    match kind {
        "image" => IconName::GalleryVerticalEnd,
        "file" => IconName::File,
        _ => IconName::CaseSensitive,
    }
}

fn empty_state(icon: IconName, title: &str, detail: &str) -> impl IntoElement {
    div()
        .py_10()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .min_w(px(0.0))
        .child(icon_tile(icon, false))
        .child(
            div()
                .mt_3()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tone_text())
                .text_center()
                .truncate()
                .child(title.to_string()),
        )
        .child(
            div()
                .mt_1()
                .text_sm()
                .text_color(tone_muted())
                .text_center()
                .line_clamp(2)
                .child(detail.to_string()),
        )
}

fn parse_args() -> anyhow::Result<(String, String)> {
    let mut args = std::env::args().skip(1);
    let mut control = None;
    let mut token = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--control" => control = args.next(),
            "--token" => token = args.next(),
            _ => {}
        }
    }
    Ok((
        control.ok_or_else(|| anyhow::anyhow!("missing --control"))?,
        token.ok_or_else(|| anyhow::anyhow!("missing --token"))?,
    ))
}

fn main() -> anyhow::Result<()> {
    let (base_url, token) = parse_args()?;
    Application::new()
        .with_assets(control_assets())
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1060.0), px(720.0)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("lanclip".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_background: WindowBackgroundAppearance::Blurred,
                    ..Default::default()
                },
                |window, cx| {
                    let app = cx.new(|_cx| ControlApp::new(base_url.clone(), token.clone()));
                    cx.new(|cx| Root::new(app, window, cx))
                },
            )
            .unwrap();
        });
    Ok(())
}
