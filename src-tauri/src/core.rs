//! mihomo 内核：启动、停止、异常退出自动重启、重新加载配置。

use crate::check::{api_client, api_get, api_url, str_of, with_auth, Api};
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct Core {
    child: Mutex<Option<Child>>,
    /// 用户希望内核运行（异常退出时据此判断要不要重启）
    want_running: AtomicBool,
    restarts: Mutex<Vec<Instant>>,
    pub last_error: Mutex<Option<String>>,
    workdir: PathBuf,
    binary: Option<PathBuf>,
    /// 这次启动改用的内核文件（Mac 增强模式下用已授权的那份）
    launch_override: Mutex<Option<PathBuf>>,
}

pub fn exe_name() -> &'static str {
    if cfg!(windows) {
        "mihomo.exe"
    } else {
        "mihomo"
    }
}

/// 内核文件的位置：打包后和主程序放在同一目录；开发时可用环境变量 CFY_MIHOMO 指定。
pub fn find_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CFY_MIHOMO") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = dir.join(exe_name());
    if p.exists() {
        return Some(p);
    }
    None
}

impl Core {
    pub fn new(workdir: PathBuf) -> Arc<Core> {
        let core = Arc::new(Core {
            child: Mutex::new(None),
            want_running: AtomicBool::new(false),
            restarts: Mutex::new(vec![]),
            last_error: Mutex::new(None),
            workdir,
            binary: find_binary(),
            launch_override: Mutex::new(None),
        });
        let watcher = core.clone();
        std::thread::spawn(move || watcher.supervise());
        core
    }

    /// 下次启动内核时改用另一个文件；None 恢复用安装包里的
    pub fn set_launch_binary(&self, p: Option<PathBuf>) {
        *self.launch_override.lock().unwrap() = p;
    }

    pub fn binary(&self) -> Option<&Path> {
        self.binary.as_deref()
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn config_path(&self) -> PathBuf {
        self.workdir.join("config.yaml")
    }

    fn log_path(&self) -> PathBuf {
        self.workdir.join("core.log")
    }

    pub fn is_running(&self) -> bool {
        let mut g = self.child.lock().unwrap();
        match g.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn spawn(&self) -> Result<(), String> {
        let over = self.launch_override.lock().unwrap().clone();
        let app = crate::edition::name();
        let bin = over.or_else(|| self.binary.clone()).ok_or_else(|| {
            if cfg!(windows) {
                tr!("找不到代理内核文件 mihomo.exe。可能被杀毒软件删除了：请在杀毒软件里恢复它并加入白名单，或重新安装 {app}。", "Can't find the proxy core file mihomo.exe. Your antivirus may have removed it: restore it and add it to the allow list, or reinstall {app}.")
            } else {
                tr!("找不到代理内核文件 mihomo，请重新安装 {app}。", "Can't find the proxy core file mihomo. Please reinstall {app}.")
            }
        })?;
        // 上次软件被强制结束（崩溃、强制退出）时留下的内核还在运行：它占着端口和增强模式的路由，
        // 新内核的增强模式会启动失败、电脑断网。先把它关掉。
        self.kill_leftovers();
        let log = File::create(self.log_path()).map_err(|e| tr!("无法创建内核日志：{e}", "Can't create the core log: {e}"))?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&bin);
        cmd.arg("-d")
            .arg(&self.workdir)
            .arg("-f")
            .arg(self.config_path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2));
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：不弹黑色命令行窗口
        let child = cmd.spawn().map_err(|e| {
            if cfg!(windows) {
                tr!("无法启动代理内核：{e}。如果装了杀毒软件，请把 {app} 的安装目录加入白名单。", "Can't start the proxy core: {e}. If you have antivirus software, add the {app} install folder to its allow list.")
            } else {
                tr!("无法启动代理内核：{e}", "Can't start the proxy core: {e}")
            }
        })?;
        let _ = std::fs::write(self.pid_path(), child.id().to_string());
        *self.child.lock().unwrap() = Some(child);
        Ok(())
    }

    fn pid_path(&self) -> PathBuf {
        self.workdir.join("mihomo.pid")
    }

    /// 关掉本软件以前启动、但没有正常退出的内核（不碰其他软件的进程）。
    /// 依据：记录的进程号；Mac 上再按命令行里本软件的配置文件路径找一遍（兼容没有进程号记录的旧版本）。
    pub fn kill_leftovers(&self) {
        let own = self.child.lock().unwrap().as_ref().map(|c| c.id());
        let mut pids: Vec<u32> = vec![];
        if let Some(p) = std::fs::read_to_string(self.pid_path()).ok().and_then(|t| t.trim().parse().ok()) {
            if is_our_core(p, &self.config_path()) {
                pids.push(p);
            }
        }
        #[cfg(unix)]
        {
            let cfg = self.config_path().to_string_lossy().to_string();
            if let Ok(out) = Command::new("ps").args(["-axo", "pid=,command="]).output() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let line = line.trim();
                    let Some((pid, cmd)) = line.split_once(' ') else { continue };
                    if cmd.contains("mihomo") && cmd.contains(&cfg) {
                        if let Ok(p) = pid.parse::<u32>() {
                            if !pids.contains(&p) {
                                pids.push(p);
                            }
                        }
                    }
                }
            }
        }
        pids.retain(|p| Some(*p) != own);
        for p in pids {
            terminate(p);
        }
        if own.is_none() {
            let _ = std::fs::remove_file(self.pid_path());
        }
    }

    /// 启动内核并等它的控制接口就绪
    pub async fn start(&self, api: &Api) -> Result<(), String> {
        if self.is_running() {
            return Ok(());
        }
        *self.last_error.lock().unwrap() = None;
        self.spawn()?;
        self.want_running.store(true, Ordering::SeqCst);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(15) {
            if !self.is_running() {
                self.want_running.store(false, Ordering::SeqCst);
                let msg = explain_log(&self.log_tail());
                *self.last_error.lock().unwrap() = Some(msg.clone());
                return Err(msg);
            }
            if api_get(api, "/version").await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err(tr!("代理内核启动超时。{}", "The proxy core took too long to start. {}", explain_log(&self.log_tail())))
    }

    pub fn stop(&self) {
        self.want_running.store(false, Ordering::SeqCst);
        if let Some(mut c) = self.child.lock().unwrap().take() {
            // 先礼貌地请它退出（让它自己清理增强模式的路由），不行再强制结束
            graceful_stop(&mut c);
        }
        let _ = std::fs::remove_file(self.pid_path());
    }

    /// 后台线程：内核意外退出时自动重启；一分钟内重启超过 5 次就放弃并报错
    fn supervise(&self) {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            if !self.want_running.load(Ordering::SeqCst) || self.is_running() {
                continue;
            }
            let mut r = self.restarts.lock().unwrap();
            r.retain(|t| t.elapsed() < Duration::from_secs(60));
            if r.len() >= 5 {
                self.want_running.store(false, Ordering::SeqCst);
                *self.last_error.lock().unwrap() =
                    Some(tr!("代理内核反复退出，已停止重启。{}", "The proxy core kept exiting, so it was not restarted again. {}", explain_log(&self.log_tail())));
                continue;
            }
            r.push(Instant::now());
            drop(r);
            if let Err(e) = self.spawn() {
                *self.last_error.lock().unwrap() = Some(e);
            }
        }
    }

    pub fn log_tail(&self) -> String {
        let Ok(mut f) = File::open(self.log_path()) else { return String::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(4000)));
        let mut s = String::new();
        let _ = f.read_to_string(&mut s);
        s
    }

    /// 让内核重新读取配置文件（不中断现有连接以外的东西）
    pub async fn reload(&self, api: &Api) -> Result<(), String> {
        if !self.is_running() {
            return Ok(());
        }
        let client = api_client()?;
        let body = serde_json::json!({ "path": self.config_path().to_string_lossy() });
        let resp = with_auth(client.put(api_url(api, "/configs?force=true")), api)
            .json(&body)
            .send()
            .await
            .map_err(|e| tr!("重新加载配置失败：{e}", "Failed to reload the config: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(tr!("配置有误，内核拒绝加载：{text}", "The core rejected the config: {text}"))
        }
    }
}

/// 进程 pid 是不是本软件的内核（用这个配置文件启动的 mihomo）
#[allow(unused_variables)]
fn is_our_core(pid: u32, cfg: &Path) -> bool {
    #[cfg(unix)]
    {
        let Ok(out) = Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()]).output() else { return false };
        let cmd = String::from_utf8_lossy(&out.stdout);
        return cmd.contains("mihomo") && cmd.contains(&*cfg.to_string_lossy());
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("tasklist");
        c.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]).creation_flags(0x0800_0000);
        let Ok(out) = c.output() else { return false };
        return String::from_utf8_lossy(&out.stdout).to_ascii_lowercase().contains("mihomo");
    }
    #[allow(unreachable_code)]
    false
}

/// 结束进程：先 SIGTERM 等它清理，2 秒后还在就强制结束
fn terminate(pid: u32) {
    #[cfg(unix)]
    {
        let quiet = |sig: &str| {
            Command::new("kill")
                .args([sig, &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        let alive = || quiet("-0");
        quiet("-TERM");
        for _ in 0..20 {
            if !alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        quiet("-KILL");
        std::thread::sleep(Duration::from_millis(300));
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("taskkill");
        c.args(["/F", "/PID", &pid.to_string()]).creation_flags(0x0800_0000);
        let _ = c.status();
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn graceful_stop(c: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &c.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for _ in 0..20 {
            if matches!(c.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = c.kill();
    let _ = c.wait();
}

/// 把内核日志里常见的错误翻译成人话
pub fn explain_log(log: &str) -> String {
    let l = log.to_ascii_lowercase();
    if l.contains("address already in use") || l.contains("only one usage of each socket address") {
        return tr!("端口被占用：可能有其他代理软件（如 Clash Verge）正在运行。请先退出它，或在设置的「高级」里换一个端口。", "The port is in use: another proxy app (such as Clash Verge) may be running. Quit it first, or pick another port under Settings → Advanced.");
    }
    if l.contains("geosite") || l.contains("geoip") || l.contains("mmdb") {
        return tr!("地理数据文件缺失或损坏，国内网站分流可能不对。请重新安装，或检查网络后重试。", "The geo data files are missing or damaged, so China-site routing may be wrong. Reinstall, or check your network and try again.");
    }
    let last: Vec<&str> = log.lines().filter(|x| !x.trim().is_empty()).rev().take(3).collect();
    if last.is_empty() {
        tr!("代理内核意外退出。", "The proxy core exited unexpectedly.")
    } else {
        tr!("代理内核报错：{}", "Proxy core error: {}", last.into_iter().rev().collect::<Vec<_>>().join(" / "))
    }
}

/// PUT /proxies/{group}：切换选择组
pub async fn select(api: &Api, group: &str, name: &str) -> Result<(), String> {
    let client = api_client()?;
    let url = api_url(api, &format!("/proxies/{}", urlencoding::encode(group)));
    let resp = with_auth(client.put(url), api)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(tr!("切换失败：{}", "Switching failed: {}", resp.text().await.unwrap_or_default()))
    }
}

/// 断开目标是这些域名（含子域名）的现有连接，让新的分流马上生效
pub async fn close_connections_for(api: &Api, domains: &[String]) -> Result<usize, String> {
    if domains.is_empty() {
        return Ok(0);
    }
    let data = api_get(api, "/connections").await?;
    let conns = data.get("connections").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    let client = api_client()?;
    let mut n = 0;
    for c in conns {
        let host = str_of(&c["metadata"], "host").to_ascii_lowercase();
        if host.is_empty() {
            continue;
        }
        let hit = domains.iter().any(|d| {
            let d = d.to_ascii_lowercase();
            host == d || host.ends_with(&format!(".{d}"))
        });
        if hit {
            let id = str_of(&c, "id");
            let _ = with_auth(client.delete(api_url(api, &format!("/connections/{id}"))), api).send().await;
            n += 1;
        }
    }
    Ok(n)
}

/// 断开全部连接（换默认地区后用）
pub async fn close_all_connections(api: &Api) {
    if let Ok(client) = api_client() {
        let _ = with_auth(client.delete(api_url(api, "/connections")), api).send().await;
    }
}

/// 测一个组里所有节点的延迟：GET /group/{name}/delay
pub async fn group_delay(api: &Api, group: &str) -> Result<Value, String> {
    let path = format!(
        "/group/{}/delay?timeout=5000&url={}",
        urlencoding::encode(group),
        urlencoding::encode("https://www.gstatic.com/generate_204")
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = with_auth(client.get(api_url(api, &path)), api).send().await.map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

/// 端口是否空闲
pub fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// 从 start 开始找一个空闲端口，跳过 avoid
pub fn pick_port(start: u16, avoid: u16) -> u16 {
    (start..start.saturating_add(200)).find(|p| *p != avoid && port_free(*p)).unwrap_or(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟软件被强制结束后留下的内核：新 Core 应该能找到并关掉它。需要 CFY_MIHOMO。
    #[test]
    #[ignore]
    fn kills_leftover_core() {
        let bin = std::env::var("CFY_MIHOMO").expect("CFY_MIHOMO");
        let dir = std::env::temp_dir().join(format!("cfy-leftover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.yaml");
        std::fs::write(&cfg, "mixed-port: 17977\nlog-level: warning\nrules:\n  - MATCH,DIRECT\n").unwrap();
        let mut orphan = Command::new(&bin).arg("-d").arg(&dir).arg("-f").arg(&cfg).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert!(matches!(orphan.try_wait(), Ok(None)), "orphan should be running");
        // 另一个无关的 mihomo（其他软件的）不能被误杀
        let other_dir = dir.join("other");
        std::fs::create_dir_all(&other_dir).unwrap();
        let other_cfg = other_dir.join("config.yaml");
        std::fs::write(&other_cfg, "mixed-port: 17978\nlog-level: warning\nrules:\n  - MATCH,DIRECT\n").unwrap();
        let mut other = Command::new(&bin).arg("-d").arg(&other_dir).arg("-f").arg(&other_cfg).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(500));

        let core = Core::new(dir.clone());
        core.kill_leftovers();
        std::thread::sleep(Duration::from_millis(300));
        assert!(matches!(orphan.try_wait(), Ok(Some(_))), "leftover core should be stopped");
        assert!(matches!(other.try_wait(), Ok(None)), "other app's core must keep running");
        let _ = other.kill();
        let _ = other.wait();
    }
}
