//! 应用配置：持久化到 `config_dir/lanclip/config.json`。

use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use lanclip_domain::DeviceId;
use serde::{Deserialize, Serialize};

const PROJECT_QUALIFIER: &str = "dev";
const PROJECT_ORG: &str = "self";
const PROJECT_APP: &str = "lanclip";

/// 用户可见的配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 设备唯一 ID（自动生成，持久化跨重启稳定）。
    pub device_id: DeviceId,
    /// 设备显示名（默认 hostname）。
    pub device_name: String,
    /// 文件下载根目录（lanclip 会在此目录下按 sender 建子目录）。
    pub download_dir: PathBuf,
    /// 文件传输并发度（数据连接数）。
    #[serde(default = "default_parallelism")]
    pub transfer_parallelism: usize,
    /// 收到 `TransferOffer` 是否自动接受（自用建议 true；可在 UI 关闭）。
    #[serde(default = "default_true")]
    pub auto_accept_transfer: bool,
    /// 是否启用剪切板自动同步。
    #[serde(default = "default_true")]
    pub clipboard_sync_enabled: bool,
    /// 是否同步文本。
    #[serde(default = "default_true")]
    pub sync_text: bool,
    /// 是否同步图片。
    #[serde(default = "default_true")]
    pub sync_images: bool,
    /// 是否在本机历史显示文件/文件夹引用。
    #[serde(default = "default_true")]
    pub show_file_refs: bool,
    /// 控制台语言："zh" | "en"。
    #[serde(default = "default_language")]
    pub language: String,
    /// 是否登录后自动启动 lanclip。
    #[serde(default)]
    pub launch_at_login: bool,
    /// 已通过确认码配对的设备。
    #[serde(default)]
    pub trusted_peers: Vec<DeviceId>,
}

fn default_parallelism() -> usize {
    lanclip_transfer::DEFAULT_PARALLELISM
}

fn default_true() -> bool {
    true
}

fn default_language() -> String {
    "zh".to_string()
}

impl AppConfig {
    /// 加载 config.json；不存在则创建默认并保存。
    pub fn load_or_create() -> anyhow::Result<Self> {
        let path = Self::config_path()?;
        if path.exists() {
            let s = fs::read_to_string(&path)?;
            let cfg: AppConfig = serde_json::from_str(&s)?;
            Ok(cfg)
        } else {
            let cfg = Self::default_new()?;
            cfg.save()?;
            Ok(cfg)
        }
    }

    /// 写回 disk。
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(self)?;
        fs::write(&path, s)?;
        Ok(())
    }

    fn default_new() -> anyhow::Result<Self> {
        let device_name = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "lanclip-device".to_string());
        let download_dir = default_download_dir();
        Ok(Self {
            device_id: DeviceId::new(),
            device_name,
            download_dir,
            transfer_parallelism: default_parallelism(),
            auto_accept_transfer: true,
            clipboard_sync_enabled: true,
            sync_text: true,
            sync_images: true,
            show_file_refs: true,
            language: default_language(),
            launch_at_login: false,
            trusted_peers: Vec::new(),
        })
    }

    fn config_path() -> anyhow::Result<PathBuf> {
        let dirs = ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORG, PROJECT_APP)
            .ok_or_else(|| anyhow::anyhow!("cannot resolve project dirs"))?;
        Ok(dirs.config_dir().join("config.json"))
    }

    pub fn config_dir() -> anyhow::Result<PathBuf> {
        let dirs = ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORG, PROJECT_APP)
            .ok_or_else(|| anyhow::anyhow!("cannot resolve project dirs"))?;
        Ok(dirs.config_dir().to_path_buf())
    }

    pub fn cache_dir() -> anyhow::Result<PathBuf> {
        let dirs = ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORG, PROJECT_APP)
            .ok_or_else(|| anyhow::anyhow!("cannot resolve project dirs"))?;
        Ok(dirs.cache_dir().to_path_buf())
    }
}

fn default_download_dir() -> PathBuf {
    // 优先 ~/Downloads/lanclip，回退到 cache_dir
    if let Some(home) = std::env::var_os("HOME") {
        let p = Path::new(&home).join("Downloads").join("lanclip");
        return p;
    }
    PathBuf::from("./lanclip-downloads")
}
