//! 软件内自动更新：从 GitHub 最新发布的 latest.json 检查新版本，一键下载、安装并重启。
//! - 安装包有签名（打包时用私钥签，软件里内置公钥校验），被篡改的安装包不会被安装。
//! - 连接中时经过本软件的代理下载（国内直连 GitHub 经常很慢或打不开）。
//! - 安装前先断开（恢复系统代理），更新后重新打开时自动连上。

use crate::app::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// 显示用的版本号，如 v6714
    pub version: String,
    /// 更新说明（发布页的说明）
    pub notes: String,
    /// 正在下载 / 安装
    pub installing: bool,
    /// 已下载字节数和总大小（总大小未知时为 0）
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}

static FOUND: Mutex<Option<(Update, UpdateInfo)>> = Mutex::new(None);
static INSTALLING: AtomicBool = AtomicBool::new(false);
static DOWNLOADED: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);

/// 软件版本 0.0.6714 → 显示为 v6714（发布标签 v6714 打包时写成 0.0.6714，见打包流程）
pub fn display_version(v: &str) -> String {
    match v.strip_prefix("0.0.") {
        Some(n) if !n.is_empty() => format!("v{n}"),
        _ => format!("v{v}"),
    }
}

/// 界面用：当前发现的新版本和下载进度
pub fn available() -> Option<UpdateInfo> {
    let found = FOUND.lock().unwrap();
    let (_, info) = found.as_ref()?;
    let mut info = info.clone();
    info.installing = INSTALLING.load(Ordering::SeqCst);
    info.downloaded = DOWNLOADED.load(Ordering::SeqCst);
    info.total = TOTAL.load(Ordering::SeqCst);
    Some(info)
}

fn set_error(e: String) {
    if let Some((_, info)) = FOUND.lock().unwrap().as_mut() {
        info.error = Some(e);
    }
}

/// 连接中时让更新请求经过本软件的代理
fn proxy_url(app: &AppHandle) -> Option<url::Url> {
    let st = app.state::<AppState>();
    if !(st.connected.load(Ordering::SeqCst) && st.core.is_running()) {
        return None;
    }
    let port = st.settings.lock().unwrap().mixed_port;
    url::Url::parse(&format!("http://127.0.0.1:{port}")).ok()
}

/// 检查一次。有新版本返回 Some。本地开发版不会检查。
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    if crate::app::app_version() == "开发版" {
        return Err("开发版不检查更新。".into());
    }
    if INSTALLING.load(Ordering::SeqCst) {
        return Ok(available());
    }
    let mut b = app.updater_builder().timeout(Duration::from_secs(60));
    if let Some(p) = proxy_url(app) {
        b = b.proxy(p);
    }
    let up =
        b.build().map_err(|e| format!("无法检查更新：{e}"))?.check().await.map_err(|e| format!("检查更新失败：{e}"))?;
    let Some(up) = up else {
        *FOUND.lock().unwrap() = None;
        return Ok(None);
    };
    let info = UpdateInfo {
        version: display_version(&up.version),
        notes: up.body.clone().unwrap_or_default().trim().to_string(),
        installing: false,
        downloaded: 0,
        total: 0,
        error: None,
    };
    *FOUND.lock().unwrap() = Some((up, info));
    Ok(available())
}

/// 下载并安装找到的新版本，然后重启软件。成功时不会返回（软件已重启）。
pub async fn install(app: &AppHandle) -> Result<(), String> {
    // 重新检查一次：按现在的连接状态决定是否经过代理下载（发现新版本时可能还没连接）
    if !INSTALLING.load(Ordering::SeqCst) {
        let _ = check(app).await;
    }
    let up = FOUND.lock().unwrap().as_ref().map(|(u, _)| u.clone()).ok_or("没有可安装的新版本。")?;
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("正在更新中。".into());
    }
    set_error_clear();
    let r = do_install(app, up).await;
    INSTALLING.store(false, Ordering::SeqCst);
    if let Err(e) = &r {
        set_error(e.clone());
    }
    r
}

fn set_error_clear() {
    DOWNLOADED.store(0, Ordering::SeqCst);
    TOTAL.store(0, Ordering::SeqCst);
    if let Some((_, info)) = FOUND.lock().unwrap().as_mut() {
        info.error = None;
    }
}

async fn do_install(app: &AppHandle, up: Update) -> Result<(), String> {
    // 下载时保持连接（下载要经过代理）
    let bytes = up
        .download(
            |chunk, total| {
                DOWNLOADED.fetch_add(chunk as u64, Ordering::SeqCst);
                if let Some(t) = total {
                    TOTAL.store(t, Ordering::SeqCst);
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("下载新版本失败：{e}"))?;

    // 下载完：先断开、恢复系统代理。Windows 的安装程序会直接结束本软件，来不及在退出时恢复，
    // 不先恢复的话电脑会一直指向已经不存在的代理，上不了网。
    let st = app.state::<AppState>();
    let _g = st.op.lock().await;
    let was_connected = st.connected.load(Ordering::SeqCst) && st.core.is_running();
    st.settings.lock().unwrap().resume_after_update = was_connected;
    let _ = st.save_settings();
    let _ = st.disconnect(false);

    if let Err(e) = up.install(bytes) {
        // 没装上：恢复原来的状态
        st.settings.lock().unwrap().resume_after_update = false;
        let _ = st.save_settings();
        if was_connected {
            let _ = st.connect().await;
        }
        return Err(format!("安装新版本失败：{e}"));
    }
    // Mac：新版本已替换好，重启生效。Windows 走不到这里（安装程序接管，装完自动打开新版本）
    app.restart();
}

/// 后台定时检查：启动 1 分钟后检查一次，之后每 6 小时一次。失败（比如没连上、GitHub 打不开）就等下次。
pub fn spawn_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            let _ = check(&app).await;
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(display_version("0.0.6714"), "v6714");
        assert_eq!(display_version("1.2.3"), "v1.2.3");
    }
}
