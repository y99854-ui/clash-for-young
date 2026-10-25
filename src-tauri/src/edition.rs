//! 同一套代码打包两个软件：Clash for Young（默认）和 Clash for Research（科研版）。
//! 打包时用环境变量 CFY_EDITION=research 选择科研版，其余一律是 Young。
//! 两个软件的名字、数据目录（由 tauri 的 identifier 决定）、端口、增强模式授权目录、更新渠道都分开，
//! 可以装在同一台电脑上互不干扰。

use std::sync::atomic::{AtomicU8, Ordering};

/// 测试用：0 = 按打包设置，1 = 强制 Young，2 = 强制 Research
static OVERRIDE: AtomicU8 = AtomicU8::new(0);

pub fn is_research() -> bool {
    match OVERRIDE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => option_env!("CFY_EDITION") == Some("research"),
    }
}

#[cfg(test)]
pub fn set_research_for_test(research: Option<bool>) {
    OVERRIDE.store(
        match research {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        },
        Ordering::Relaxed,
    );
}

/// 界面上用的名字
pub fn name() -> &'static str {
    if is_research() {
        "Clash for Research"
    } else {
        "Clash for Young"
    }
}

/// 给界面的标识："young" / "research"
pub fn id() -> &'static str {
    if is_research() {
        "research"
    } else {
        "young"
    }
}

/// 下载页（分享按钮复制的链接）
pub fn download_url() -> &'static str {
    if is_research() {
        "https://github.com/y99854-ui/clash-for-research/releases/latest"
    } else {
        "https://github.com/y99854-ui/clash-for-young/releases/latest"
    }
}

/// Mac 增强模式：授权后的内核副本放在这里（两个软件各用各的，互不覆盖）
pub fn mac_priv_dir() -> &'static str {
    if is_research() {
        "/Library/Application Support/ClashForResearch"
    } else {
        "/Library/Application Support/ClashForYoung"
    }
}

/// 默认端口：两个软件同时运行也不冲突
pub fn default_ports() -> (u16, u16) {
    if is_research() {
        (7900, 9099)
    } else {
        (7899, 9098)
    }
}
