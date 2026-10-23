//! 系统代理：连接时指向本软件，断开或退出时恢复原样。
//! 原来的设置存在 sysproxy-backup.json，软件崩溃后下次启动也能恢复。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use sysproxy::Sysproxy;

const HOST: &str = "127.0.0.1";

#[cfg(target_os = "windows")]
const BYPASS: &str = "localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;192.168.*;<local>";
#[cfg(not(target_os = "windows"))]
const BYPASS: &str = "127.0.0.1,192.168.0.0/16,10.0.0.0/8,172.16.0.0/12,localhost,*.local,timestamp.apple.com";

#[derive(Serialize, Deserialize, Default)]
struct Backup {
    enable: bool,
    host: String,
    port: u16,
    bypass: String,
}

fn backup_path(dir: &Path) -> PathBuf {
    dir.join("sysproxy-backup.json")
}

fn is_ours(p: &Sysproxy, port: u16) -> bool {
    p.enable && p.port == port && (p.host == HOST || p.host == "localhost")
}

/// 系统代理当前是否指向本软件
pub fn is_on(port: u16) -> bool {
    Sysproxy::get_system_proxy().map(|p| is_ours(&p, port)).unwrap_or(false)
}

pub fn enable(dir: &Path, port: u16) -> Result<(), String> {
    let cur = Sysproxy::get_system_proxy().unwrap_or_default();
    // 记下原来的设置（如果已经是我们的，就别覆盖备份）
    if !is_ours(&cur, port) {
        let b = Backup { enable: cur.enable, host: cur.host.clone(), port: cur.port, bypass: cur.bypass.clone() };
        if let Ok(t) = serde_json::to_string(&b) {
            let _ = std::fs::write(backup_path(dir), t);
        }
    }
    let p = Sysproxy { enable: true, host: HOST.into(), port, bypass: BYPASS.into() };
    p.set_system_proxy().map_err(|e| crate::tr!("设置系统代理失败：{e}", "Failed to set the system proxy: {e}"))
}

/// 恢复连接前的系统代理设置；没有备份就直接关闭
pub fn disable(dir: &Path, port: u16) -> Result<(), String> {
    let backup: Option<Backup> =
        std::fs::read_to_string(backup_path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok());
    let restore = match backup {
        // 原来就开着别的代理（而且不是我们自己）：还原它
        Some(b) if b.enable && !(b.port == port && b.host == HOST) => {
            Sysproxy { enable: true, host: b.host, port: b.port, bypass: b.bypass }
        }
        Some(b) => {
            let bypass = if b.bypass.is_empty() { BYPASS.to_string() } else { b.bypass };
            let host = if b.host.is_empty() { HOST.to_string() } else { b.host };
            Sysproxy { enable: false, host, port: if b.port == 0 { port } else { b.port }, bypass }
        }
        None => Sysproxy { enable: false, host: HOST.into(), port, bypass: BYPASS.into() },
    };
    let r = restore.set_system_proxy().map_err(|e| crate::tr!("恢复系统代理失败：{e}", "Failed to restore the system proxy: {e}"));
    let _ = std::fs::remove_file(backup_path(dir));
    r
}

/// 启动时检查：上次没正常退出、系统代理还指着我们，而内核没在运行 → 恢复，免得断网
pub fn recover_on_startup(dir: &Path, port: u16) -> bool {
    if is_on(port) {
        let _ = disable(dir, port);
        return true;
    }
    false
}
