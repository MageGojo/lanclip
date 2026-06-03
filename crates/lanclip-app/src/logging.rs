//! 日志初始化：默认 INFO，`RUST_LOG` 环境变量可覆盖。

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// 初始化全局日志订阅器。**仅调用一次**（通常在 main 起手）。
pub fn init() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,lanclip=debug"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_thread_names(false),
        )
        .init();
}
