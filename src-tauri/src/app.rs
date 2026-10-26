//! 软件状态和界面调用的全部命令。

use crate::check::{self, Api, CheckResult};
use crate::configgen::{self, GenInput, GROUP_DEFAULT};
use crate::core::{self, Core};
use crate::settings::{self, CustomRule, CustomSite, NodeGroup, Settings, Subscription};
use crate::sites::{self, Site};
use crate::subscription::{self, Parsed, Region};
use crate::sysproxy_ctl;
use crate::tun;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

pub struct AppState {
    pub dir: PathBuf,
    pub settings: Mutex<Settings>,
    pub parsed: Mutex<Parsed>,
    pub core: Arc<Core>,
    pub geo_resource_dir: Option<PathBuf>,
    pub connected: AtomicBool,
    /// 修改类操作排队执行，避免两次写配置互相覆盖
    pub op: tokio::sync::Mutex<()>,
}

const GEO_FILES: &[(&str, &str)] = &[
    // (内核要的文件名, 下载地址里的文件名)
    ("GeoSite.dat", "geosite.dat"),
    ("geoip.metadb", "country-lite.mmdb"),
];
const GEO_MIRRORS: &[&str] = &[
    "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/",
    "https://fastly.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/",
    "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/",
];

fn sub_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("subscription-{id}.yaml"))
}

fn host_of(url: &str) -> String {
    url::Url::parse(url.trim()).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| tr!("订阅", "Subscription"))
}

/// 旧版只有一个订阅、没有编号，文件名是 subscription-0.yaml：补上编号、名字和节点数
fn migrate_subscriptions(dir: &Path, s: &mut Settings) {
    for (i, sub) in s.subscriptions.iter_mut().enumerate() {
        if sub.id.is_empty() {
            sub.id = format!("s{i}");
            let old = dir.join(format!("subscription-{i}.yaml"));
            if old.exists() {
                let _ = std::fs::rename(&old, sub_file(dir, &sub.id));
            }
        }
        if sub.name.is_empty() {
            sub.name = if sub.url.is_empty() { tr!("导入的订阅", "Imported") } else { host_of(&sub.url) };
        }
        if sub.node_count == 0 {
            if let Ok(t) = std::fs::read_to_string(sub_file(dir, &sub.id)) {
                sub.node_count = subscription::validate(&t).unwrap_or(0);
            }
        }
    }
}

impl AppState {
    pub fn new(dir: PathBuf, geo_resource_dir: Option<PathBuf>) -> AppState {
        let _ = std::fs::create_dir_all(dir.join("core"));
        let mut settings = settings::load(&dir);
        // 先定语言，再定默认分组的名字（“美国”或 “United States”）；之后名字固定，不随语言变，
        // 免得网站页里选过的分组失效
        #[cfg(not(test))]
        crate::i18n::apply(&settings.language);
        if settings.groups.is_none() {
            settings.groups = Some(subscription::default_groups());
        }
        migrate_subscriptions(&dir, &mut settings);
        let _ = settings::save(&dir, &settings);
        let core = Core::new(dir.join("core"));
        let app = AppState {
            dir,
            settings: Mutex::new(settings),
            parsed: Mutex::new(Parsed::default()),
            core,
            geo_resource_dir,
            connected: AtomicBool::new(false),
            op: tokio::sync::Mutex::new(()),
        };
        app.reparse();
        app
    }

    pub fn api(&self) -> Api {
        let s = self.settings.lock().unwrap();
        Api {
            controller: format!("127.0.0.1:{}", s.controller_port),
            secret: s.secret.clone(),
            mixed_port: s.mixed_port,
        }
    }

    pub(crate) fn save_settings(&self) -> Result<(), String> {
        let s = self.settings.lock().unwrap().clone();
        settings::save(&self.dir, &s)
    }

    fn geo_ready(&self) -> bool {
        GEO_FILES.iter().all(|(n, _)| self.core.workdir().join(n).exists())
    }

    /// 准备地理数据：先用安装包里带的，没有再下载
    async fn ensure_geo(&self) -> bool {
        let wd = self.core.workdir().to_path_buf();
        for (name, remote) in GEO_FILES {
            let dst = wd.join(name);
            if dst.exists() {
                continue;
            }
            if let Some(res) = &self.geo_resource_dir {
                for cand in [res.join(name), res.join(remote)] {
                    if cand.exists() && std::fs::copy(&cand, &dst).is_ok() {
                        break;
                    }
                }
            }
            if dst.exists() {
                continue;
            }
            let client = match reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(60)).build() {
                Ok(c) => c,
                Err(_) => continue,
            };
            for m in GEO_MIRRORS {
                if let Ok(r) = client.get(format!("{m}{remote}")).send().await {
                    if r.status().is_success() {
                        if let Ok(b) = r.bytes().await {
                            if b.len() > 1000 {
                                let tmp = wd.join(format!("{name}.part"));
                                if std::fs::write(&tmp, &b).is_ok() && std::fs::rename(&tmp, &dst).is_ok() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        self.geo_ready()
    }

    /// 按当前设置生成配置文件。返回走向失效的条目。
    fn write_config(&self) -> Result<Vec<String>, String> {
        let st = self.settings.lock().unwrap().clone();
        let parsed = self.parsed.lock().unwrap().clone();
        let sites = sites::catalog(&st);
        let out = configgen::generate(&GenInput {
            settings: &st,
            parsed: &parsed,
            sites: &sites,
            geo_ready: self.geo_ready(),
            tun: self.tun_active(),
        });
        let path = self.core.config_path();
        let tmp = path.with_extension("yaml.tmp");
        std::fs::write(&tmp, out.yaml).map_err(|e| tr!("无法写入配置：{e}", "Can't write the config file: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| tr!("无法写入配置：{e}", "Can't write the config file: {e}"))?;
        Ok(out.fallbacks)
    }

    /// 写配置并让正在运行的内核重新加载；domains 里的网站断开旧连接
    async fn apply(&self, domains: &[String]) -> Result<Vec<String>, String> {
        let fallbacks = self.write_config()?;
        let api = self.api();
        if self.core.is_running() {
            self.core.reload(&api).await?;
            let _ = core::close_connections_for(&api, domains).await;
        }
        Ok(fallbacks)
    }

    /// 内核没运行时，确认端口可用，被占用就换一个
    fn ensure_ports(&self) -> Result<(), String> {
        let mut s = self.settings.lock().unwrap();
        let mut changed = false;
        if !core::port_free(s.mixed_port) {
            s.mixed_port = core::pick_port(crate::edition::default_ports().0, s.controller_port);
            changed = true;
        }
        if !core::port_free(s.controller_port) {
            s.controller_port = core::pick_port(crate::edition::default_ports().1, s.mixed_port);
            changed = true;
        }
        let snapshot = s.clone();
        drop(s);
        if changed {
            settings::save(&self.dir, &snapshot)?;
        }
        Ok(())
    }

    async fn start_core(&self) -> Result<(), String> {
        if self.core.is_running() {
            return Ok(());
        }
        if self.parsed.lock().unwrap().proxies.is_empty() {
            return Err(tr!("还没有添加订阅。", "No subscription added yet."));
        }
        self.ensure_ports()?;
        self.ensure_geo().await;
        // Mac 增强模式：用已授权的内核副本启动
        self.core.set_launch_binary(if self.tun_active() { tun::launch_binary() } else { None });
        self.write_config()?;
        self.core.start(&self.api()).await
    }

    pub async fn connect(&self) -> Result<(), String> {
        self.start_core().await?;
        let port = self.settings.lock().unwrap().mixed_port;
        sysproxy_ctl::enable(&self.dir, port)?;
        self.connected.store(true, Ordering::SeqCst);
        if self.terminal_proxy_on() {
            if let Err(e) = crate::termproxy::enable(&self.dir, port) {
                *self.core.last_error.lock().unwrap() = Some(e);
            }
        }
        self.settings.lock().unwrap().was_connected = true;
        self.save_settings()
    }

    /// 断开：恢复系统代理、停止内核。remember=false 用于退出软件
    pub fn disconnect(&self, remember: bool) -> Result<(), String> {
        let port = self.settings.lock().unwrap().mixed_port;
        let was = self.connected.swap(false, Ordering::SeqCst);
        let r = if was || sysproxy_ctl::is_on(port) { sysproxy_ctl::disable(&self.dir, port) } else { Ok(()) };
        self.core.stop();
        let _ = crate::termproxy::disable(&self.dir);
        if remember {
            self.settings.lock().unwrap().was_connected = false;
            let _ = self.save_settings();
        }
        r
    }

    /// 科研版的终端代理是否打开
    fn terminal_proxy_on(&self) -> bool {
        crate::edition::is_research() && self.settings.lock().unwrap().terminal_proxy
    }

    /// 增强模式是否真正生效（打开了且有权限）
    fn tun_active(&self) -> bool {
        self.settings.lock().unwrap().tun_enabled && tun::privileged(self.core.binary())
    }

    /// 重启内核让新设置生效（TUN 开关需要重启），保持原来的连接状态
    async fn restart_core(&self) -> Result<(), String> {
        if !self.core.is_running() {
            return self.write_config().map(|_| ());
        }
        let was_connected = self.connected.load(Ordering::SeqCst);
        self.core.stop();
        if was_connected {
            self.connect().await
        } else {
            self.start_core().await
        }
    }

    /// 读取全部订阅文件，合并节点、重新分组
    fn reparse(&self) {
        let (subs, defs) = {
            let s = self.settings.lock().unwrap();
            (s.subscriptions.clone(), s.node_groups())
        };
        let texts: Vec<String> =
            subs.iter().filter_map(|s| std::fs::read_to_string(sub_file(&self.dir, &s.id)).ok()).collect();
        *self.parsed.lock().unwrap() = subscription::parse_many(&texts, &defs);
    }

    /// 订阅增删改之后：重新合并节点，生成配置；一个节点都没有了就断开
    async fn after_subs_changed(&self) -> Result<(), String> {
        self.reparse();
        if self.parsed.lock().unwrap().proxies.is_empty() {
            if self.core.is_running() {
                self.disconnect(true)?;
            }
            return Ok(());
        }
        if self.core.is_running() {
            self.apply(&[]).await?;
        } else {
            self.write_config()?;
        }
        Ok(())
    }

    /// 下载（或重新下载）一个链接订阅。同一个链接已存在就更新它
    async fn fetch_url_sub(&self, url: &str) -> Result<(), String> {
        let url = url.trim();
        let via = if self.core.is_running() { Some(self.settings.lock().unwrap().mixed_port) } else { None };
        let fetched = subscription::fetch(url, via).await?;
        let n = subscription::validate(&fetched.text)?;
        let id = self
            .settings
            .lock()
            .unwrap()
            .subscriptions
            .iter()
            .find(|s| s.url == url)
            .map(|s| s.id.clone())
            .unwrap_or_else(|| format!("s{}", settings::now_ms()));
        std::fs::write(sub_file(&self.dir, &id), &fetched.text).map_err(|e| tr!("无法保存订阅：{e}", "Can't save the subscription: {e}"))?;
        {
            let mut s = self.settings.lock().unwrap();
            match s.subscriptions.iter_mut().find(|x| x.id == id) {
                Some(sub) => {
                    sub.updated_at = settings::now_ms();
                    sub.usage = fetched.usage;
                    sub.node_count = n;
                }
                None => s.subscriptions.push(Subscription {
                    id,
                    name: host_of(url),
                    url: url.to_string(),
                    node_count: n,
                    updated_at: settings::now_ms(),
                    usage: fetched.usage,
                }),
            }
        }
        self.save_settings()
    }

    /// 从文件导入订阅（Clash 配置文件或订阅内容）
    fn import_file_sub(&self, file_name: &str, content: &str) -> Result<(), String> {
        let n = subscription::validate(content)?;
        let id = format!("s{}", settings::now_ms());
        std::fs::write(sub_file(&self.dir, &id), content).map_err(|e| tr!("无法保存订阅：{e}", "Can't save the subscription: {e}"))?;
        let name = file_name.rsplit_once('.').map(|(a, _)| a).unwrap_or(file_name).trim();
        self.settings.lock().unwrap().subscriptions.push(Subscription {
            id,
            name: if name.is_empty() { tr!("导入的订阅", "Imported") } else { name.to_string() },
            url: String::new(),
            node_count: n,
            updated_at: settings::now_ms(),
            usage: None,
        });
        self.save_settings()
    }

    /// 更新所有链接订阅；返回失败的（名字，原因）
    async fn refresh_all(&self) -> Vec<(String, String)> {
        let subs = self.settings.lock().unwrap().subscriptions.clone();
        let mut failed = vec![];
        for s in subs.iter().filter(|s| !s.url.is_empty()) {
            if let Err(e) = self.fetch_url_sub(&s.url).await {
                failed.push((s.name.clone(), e));
            }
        }
        failed
    }
}

// ───────────────────────── 给界面的数据 ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteView {
    #[serde(flatten)]
    pub site: Site,
    /// 用户选的走向，"" 表示自动
    pub choice: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    #[serde(flatten)]
    pub def: NodeGroup,
    pub flag: String,
    /// 目前归入这个分组的节点
    pub matched: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunView {
    /// 用户打开了增强模式
    pub enabled: bool,
    /// 内核有权限（Mac 已授权 / Windows 以管理员身份运行）
    pub privileged: bool,
    /// mac / windows / linux
    pub platform: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateView {
    pub subscriptions: Vec<Subscription>,
    /// 有节点的分组（可选的走向）
    pub regions: Vec<Region>,
    /// 全部分组的定义和匹配结果（分组管理用）
    pub groups: Vec<GroupView>,
    pub node_count: usize,
    pub default_target: String,
    pub sites: Vec<SiteView>,
    pub custom_rules: Vec<CustomRule>,
    pub running: bool,
    pub connected: bool,
    pub mixed_port: u16,
    pub controller_port: u16,
    pub error: Option<String>,
    /// 走向已失效（订阅里没有了）的条目
    pub fallbacks: Vec<String>,
    pub geo_ready: bool,
    pub tun: TunView,
    pub data_dir: String,
    /// 版本号（发版时由打包流程写入，如 v6712；本地开发是“开发版”）
    pub version: String,
    /// 全局模式（所有网站都走默认地区）
    pub global_mode: bool,
    /// 发现的新版本（没有为 None）
    pub update: Option<crate::updater::UpdateInfo>,
    /// 语言设置：""（跟随系统）/ "zh" / "en"
    pub language: String,
    /// 实际使用的语言："zh" / "en"
    pub lang: String,
    /// "young" / "research"
    pub edition: String,
    pub app_name: String,
    pub download_url: String,
    /// 科研版：我在校园网
    pub campus_mode: bool,
    /// 科研版：终端代理
    pub terminal_proxy: bool,
}

/// 本地开发时的版本号（界面显示为“开发版”）
pub const DEV_VERSION: &str = "dev";

/// 版本号：GitHub Actions 打包时把发布标签放进环境变量 CFY_VERSION
pub fn app_version() -> String {
    option_env!("CFY_VERSION").filter(|v| !v.trim().is_empty()).unwrap_or(DEV_VERSION).to_string()
}

fn state_view(app: &AppState) -> StateView {
    let st = app.settings.lock().unwrap().clone();
    let parsed = app.parsed.lock().unwrap().clone();
    let sites = sites::catalog(&st);
    let fallbacks =
        configgen::generate(&GenInput { settings: &st, parsed: &parsed, sites: &sites, geo_ready: true, tun: false })
            .fallbacks;
    let sites = sites
        .into_iter()
        .map(|s| SiteView { choice: sites::choice_of(&st, &s).to_string(), site: s })
        .collect();
    let running = app.core.is_running();
    StateView {
        subscriptions: st.subscriptions.clone(),
        node_count: parsed.proxies.len(),
        regions: parsed.regions.clone(),
        groups: st
            .node_groups()
            .into_iter()
            .map(|g| {
                let matched =
                    parsed.all_groups.iter().find(|r| r.id == g.id).map(|r| r.nodes.clone()).unwrap_or_default();
                GroupView { flag: subscription::flag_of_group(&g), def: g, matched }
            })
            .collect(),
        default_target: st.default_target.clone(),
        sites,
        custom_rules: st.custom_rules.clone(),
        running,
        connected: app.connected.load(Ordering::SeqCst) && running,
        mixed_port: st.mixed_port,
        controller_port: st.controller_port,
        error: app.core.last_error.lock().unwrap().clone(),
        fallbacks,
        geo_ready: app.geo_ready(),
        tun: TunView {
            enabled: st.tun_enabled,
            privileged: tun::privileged(app.core.binary()),
            platform: tun::platform().into(),
        },
        data_dir: app.dir.to_string_lossy().to_string(),
        version: app_version(),
        global_mode: st.global_mode,
        update: crate::updater::available(),
        language: st.language.clone(),
        lang: crate::i18n::current().to_string(),
        edition: crate::edition::id().to_string(),
        app_name: crate::edition::name().to_string(),
        download_url: crate::edition::download_url().to_string(),
        campus_mode: st.campus_mode,
        terminal_proxy: st.terminal_proxy,
    }
}

// ───────────────────────── 命令 ─────────────────────────

type S<'a> = State<'a, AppState>;

#[tauri::command]
pub fn get_state(app: S) -> StateView {
    state_view(&app)
}

/// 添加订阅链接（同一个链接已存在就更新它）
#[tauri::command]
pub async fn add_subscription(app: S<'_>, url: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.fetch_url_sub(&url).await?;
    app.after_subs_changed().await?;
    Ok(state_view(&app))
}

/// 从文件导入订阅：界面读出文件内容后传过来
#[tauri::command]
pub async fn import_subscription_file(app: S<'_>, name: String, content: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.import_file_sub(&name, &content)?;
    app.after_subs_changed().await?;
    Ok(state_view(&app))
}

#[tauri::command]
pub async fn remove_subscription(app: S<'_>, id: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.settings.lock().unwrap().subscriptions.retain(|s| s.id != id);
    app.save_settings()?;
    let _ = std::fs::remove_file(sub_file(&app.dir, &id));
    app.after_subs_changed().await?;
    Ok(state_view(&app))
}

#[tauri::command]
pub async fn rename_subscription(app: S<'_>, id: String, name: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(tr!("名字不能为空。", "The name can't be empty."));
    }
    if let Some(s) = app.settings.lock().unwrap().subscriptions.iter_mut().find(|s| s.id == id) {
        s.name = name;
    }
    app.save_settings()?;
    Ok(state_view(&app))
}

/// 更新链接订阅：给 id 只更新那一个，不给就全部更新
#[tauri::command]
pub async fn refresh_subscription(app: S<'_>, id: Option<String>) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    let url_subs: Vec<Subscription> =
        app.settings.lock().unwrap().subscriptions.iter().filter(|s| !s.url.is_empty()).cloned().collect();
    if url_subs.is_empty() {
        return Err(tr!("没有可以更新的订阅链接（从文件导入的订阅需要重新导入）。", "No subscription link to update (subscriptions imported from a file need to be imported again)."));
    }
    let failed = match id {
        Some(id) => {
            let Some(s) = url_subs.iter().find(|s| s.id == id) else {
                return Err(tr!("找不到这个订阅。", "Subscription not found."));
            };
            match app.fetch_url_sub(&s.url).await {
                Ok(()) => vec![],
                Err(e) => vec![(s.name.clone(), e)],
            }
        }
        None => app.refresh_all().await,
    };
    app.after_subs_changed().await?;
    if !failed.is_empty() {
        let msg = failed.iter().map(|(n, e)| format!("「{n}」{e}")).collect::<Vec<_>>().join("\n");
        return Err(tr!("部分订阅更新失败（其他订阅已更新）：\n{msg}", "Some subscriptions failed to update (the others were updated):\n{msg}"));
    }
    Ok(state_view(&app))
}

/// 后台定时更新（不占用界面的错误提示）
pub async fn auto_refresh(app: &AppState) {
    let _g = app.op.lock().await;
    let _ = app.refresh_all().await;
    let _ = app.after_subs_changed().await;
}

#[tauri::command]
pub async fn connect(app: S<'_>) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.connect().await?;
    Ok(state_view(&app))
}

#[tauri::command]
pub async fn disconnect(app: S<'_>) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.disconnect(true)?;
    Ok(state_view(&app))
}

#[tauri::command]
pub async fn set_default_target(app: S<'_>, target: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.settings.lock().unwrap().default_target = target.clone();
    app.save_settings()?;
    app.write_config()?;
    if app.core.is_running() {
        let api = app.api();
        // 科研版：稳定线路的节点跟着默认地区变，要重新加载配置
        if crate::edition::is_research() {
            app.core.reload(&api).await?;
        }
        let resolved = {
            let parsed = app.parsed.lock().unwrap();
            configgen::resolve_target(&target, &parsed).ok().flatten()
        };
        if let Some(name) = resolved {
            core::select(&api, GROUP_DEFAULT, &name).await?;
        }
        core::close_all_connections(&api).await;
    }
    Ok(state_view(&app))
}

/// 切换界面语言：""（跟随系统）/ "zh" / "en"
#[tauri::command]
pub fn set_language(app: S, handle: tauri::AppHandle, language: String) -> Result<StateView, String> {
    let language = match language.as_str() {
        "zh" | "en" => language,
        _ => String::new(),
    };
    crate::i18n::apply(&language);
    app.settings.lock().unwrap().language = language;
    app.save_settings()?;
    crate::tray::relabel(&handle);
    Ok(state_view(&app))
}

/// 科研版：我在校园网（出版社、数据库直连）
#[tauri::command]
pub async fn set_campus_mode(app: S<'_>, on: bool) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.settings.lock().unwrap().campus_mode = on;
    app.save_settings()?;
    let domains: Vec<String> = {
        let st = app.settings.lock().unwrap();
        sites::catalog(&st).into_iter().filter(|s| s.campus).flat_map(|s| s.domains).collect()
    };
    app.apply(&domains).await?;
    Ok(state_view(&app))
}

/// 科研版：终端代理。连接中立即生效
#[tauri::command]
pub async fn set_terminal_proxy(app: S<'_>, on: bool) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.settings.lock().unwrap().terminal_proxy = on;
    app.save_settings()?;
    let connected = app.connected.load(Ordering::SeqCst) && app.core.is_running();
    let port = app.settings.lock().unwrap().mixed_port;
    if on && connected {
        crate::termproxy::enable(&app.dir, port)?;
    } else {
        crate::termproxy::disable(&app.dir)?;
    }
    Ok(state_view(&app))
}

/// 检查更新（设置页的按钮）
#[tauri::command]
pub async fn check_update(app: S<'_>, handle: tauri::AppHandle) -> Result<StateView, String> {
    crate::updater::check(&handle).await?;
    Ok(state_view(&app))
}

/// 下载安装新版本并重启。下载进度通过 get_state 里的 update 查看。
#[tauri::command]
pub async fn install_update(handle: tauri::AppHandle) -> Result<(), String> {
    crate::updater::install(&handle).await
}

/// 切换 规则 / 全局 模式。连接中立即生效：重新加载配置并断开旧连接，让它们按新模式重连
#[tauri::command]
pub async fn set_global_mode(app: S<'_>, global: bool) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.settings.lock().unwrap().global_mode = global;
    app.save_settings()?;
    if app.core.is_running() {
        app.apply(&[]).await?;
        core::close_all_connections(&app.api()).await;
    } else {
        app.write_config()?;
    }
    Ok(state_view(&app))
}

#[tauri::command]
pub async fn set_site_choice(app: S<'_>, id: String, target: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    let domains = {
        let mut s = app.settings.lock().unwrap();
        let site = sites::catalog(&s).into_iter().find(|x| x.id == id);
        let default = site.as_ref().map(|x| x.default_choice.clone()).unwrap_or_default();
        // 和默认一样就不记录（以后默认改了也跟着变）；“自动”但默认不是自动时要记下来
        if target == default {
            s.site_choices.remove(&id);
        } else {
            s.site_choices.insert(id.clone(), target);
        }
        site.map(|x| x.domains).unwrap_or_default()
    };
    app.save_settings()?;
    app.apply(&domains).await?;
    Ok(state_view(&app))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteInput {
    pub id: Option<String>,
    pub name: String,
    pub url: String,
    pub domains: Vec<String>,
}

fn clean_domains(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for d in list {
        for part in d.split(|c: char| c == ',' || c == '，' || c.is_whitespace()) {
            if let Some(n) = sites::normalize_domain(part) {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// 添加或修改网站。内置网站只能改域名。
#[tauri::command]
pub async fn save_site(app: S<'_>, site: SiteInput) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    let mut domains = clean_domains(&site.domains);
    let url_domain = sites::normalize_domain(&site.url);
    if domains.is_empty() {
        if let Some(d) = &url_domain {
            domains.push(d.clone());
        }
    }
    if domains.is_empty() {
        return Err(tr!("请至少填写一个有效的域名，例如 example.com", "Enter at least one valid domain, e.g. example.com"));
    }
    let mut all_domains = domains.clone();
    {
        let mut s = app.settings.lock().unwrap();
        let builtin = sites::builtin().iter().find(|b| Some(&b.id) == site.id.as_ref()).cloned();
        if let Some(b) = builtin {
            all_domains.extend(b.domains.clone());
            if domains == b.domains {
                s.site_domains.remove(&b.id);
            } else {
                s.site_domains.insert(b.id.clone(), domains);
            }
        } else if let Some(c) = site.id.as_ref().and_then(|id| s.custom_sites.iter_mut().find(|c| &c.id == id)) {
            all_domains.extend(c.domains.clone());
            c.name = if site.name.trim().is_empty() { domains[0].clone() } else { site.name.trim().into() };
            c.url = url_domain.clone().unwrap_or_else(|| domains[0].clone());
            c.domains = domains;
        } else {
            let name = if site.name.trim().is_empty() { domains[0].clone() } else { site.name.trim().to_string() };
            let url = url_domain.unwrap_or_else(|| domains[0].clone());
            s.custom_sites.push(CustomSite { id: format!("c{}", settings::now_ms()), name, url, domains });
        }
    }
    app.save_settings()?;
    app.apply(&all_domains).await?;
    Ok(state_view(&app))
}

/// 删除自己添加的网站；对内置网站则是恢复默认域名
#[tauri::command]
pub async fn delete_site(app: S<'_>, id: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    let domains = {
        let mut s = app.settings.lock().unwrap();
        let d = sites::catalog(&s).into_iter().find(|x| x.id == id).map(|x| x.domains).unwrap_or_default();
        if s.custom_sites.iter().any(|c| c.id == id) {
            s.custom_sites.retain(|c| c.id != id);
            s.site_choices.remove(&id);
        } else {
            s.site_domains.remove(&id);
        }
        d
    };
    app.save_settings()?;
    app.apply(&domains).await?;
    Ok(state_view(&app))
}

#[tauri::command]
pub async fn set_custom_rules(app: S<'_>, rules: Vec<CustomRule>) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    let mut cleaned = vec![];
    for r in rules {
        let mut r = r;
        r.value = r.value.trim().to_string();
        if r.value.is_empty() {
            continue;
        }
        if configgen::rule_line(&r, "DIRECT").is_none() {
            return Err(tr!("规则「{}」格式不对：不能包含逗号或空格。", "Rule \"{}\" is invalid: it can't contain commas or spaces.", r.value));
        }
        if r.kind == "ipcidr" && !looks_like_ip(&r.value) {
            return Err(tr!("「{}」不是有效的 IP 或 IP 段，例如 1.2.3.0/24", "\"{}\" is not a valid IP or IP range, e.g. 1.2.3.0/24", r.value));
        }
        cleaned.push(r);
    }
    let mut domains: Vec<String> = app.settings.lock().unwrap().custom_rules.iter().map(|r| r.value.clone()).collect();
    domains.extend(cleaned.iter().map(|r| r.value.clone()));
    app.settings.lock().unwrap().custom_rules = cleaned;
    app.save_settings()?;
    app.apply(&domains).await?;
    Ok(state_view(&app))
}

fn looks_like_ip(v: &str) -> bool {
    let (ip, mask) = match v.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (v, None),
    };
    let ok_ip = ip.parse::<std::net::IpAddr>().is_ok();
    let ok_mask = mask.map(|m| m.parse::<u8>().map(|n| n <= 128).unwrap_or(false)).unwrap_or(true);
    ok_ip && ok_mask
}

// ───────────────────────── 节点分组 ─────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInput {
    pub id: Option<String>,
    pub name: String,
    /// 关键词，逗号、空格或换行分隔
    pub keywords: String,
    pub nodes: Vec<String>,
}

fn split_keywords(s: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for k in s.split(|c: char| c == ',' || c == '，' || c == '、' || c == '\n' || c == '|') {
        let k = k.trim().to_string();
        if !k.is_empty() && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// 把设置里引用旧分组名的地方（默认地区、网站选择、自定义规则）改成新名字
fn rename_refs(s: &mut Settings, old: &str, new: &str) {
    let (o, n) = (format!("region:{old}"), format!("region:{new}"));
    if s.default_target == o {
        s.default_target = n.clone();
    }
    for v in s.site_choices.values_mut() {
        if *v == o {
            *v = n.clone();
        }
    }
    for r in s.custom_rules.iter_mut() {
        if r.target == o {
            r.target = n.clone();
        }
    }
}

impl AppState {
    fn regroup(&self) {
        let defs = self.settings.lock().unwrap().node_groups();
        self.parsed.lock().unwrap().apply_groups(&defs);
    }
}

/// 预览：按这些关键词和手动挑选的节点，会归入哪些节点
#[tauri::command]
pub fn preview_group(app: S, keywords: String, nodes: Vec<String>) -> Vec<String> {
    let g = NodeGroup { id: String::new(), name: String::new(), keywords: split_keywords(&keywords), nodes };
    app.parsed.lock().unwrap().node_names().into_iter().filter(|n| subscription::group_matches(&g, n)).collect()
}

/// 新建或修改分组
#[tauri::command]
pub async fn save_group(app: S<'_>, group: GroupInput) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    let name = group.name.trim().to_string();
    if name.is_empty() {
        return Err(tr!("请填写分组名称。", "Enter a group name."));
    }
    if name.contains(',') || name.contains('\n') {
        return Err(tr!("分组名称不能包含逗号或换行。", "Group names can't contain commas or line breaks."));
    }
    if subscription::reserved_names().contains(&name) {
        return Err(tr!("「{name}」是软件保留的名字，请换一个。", "\"{name}\" is a reserved name. Please choose another."));
    }
    if app.parsed.lock().unwrap().node_names().contains(&name) {
        return Err(tr!("「{name}」和某个节点同名，请换一个名字。", "\"{name}\" is the same as a node name. Please choose another."));
    }
    let keywords = split_keywords(&group.keywords);
    if keywords.is_empty() && group.nodes.is_empty() {
        return Err(tr!("请填写至少一个关键词，或手动选择至少一个节点。", "Enter at least one keyword, or pick at least one node."));
    }
    {
        let mut s = app.settings.lock().unwrap();
        let mut list = s.node_groups();
        if list.iter().any(|g| g.name == name && Some(&g.id) != group.id.as_ref()) {
            return Err(tr!("已经有名为「{name}」的分组了。", "A group named \"{name}\" already exists."));
        }
        let mut rename = None;
        match group.id.as_ref().and_then(|id| list.iter_mut().find(|g| &g.id == id)) {
            Some(g) => {
                if g.name != name {
                    rename = Some(g.name.clone());
                }
                g.name = name.clone();
                g.keywords = keywords;
                g.nodes = group.nodes;
            }
            None => list.push(NodeGroup {
                id: format!("g{}", settings::now_ms()),
                name: name.clone(),
                keywords,
                nodes: group.nodes,
            }),
        }
        s.groups = Some(list);
        if let Some(old) = rename {
            rename_refs(&mut s, &old, &name);
        }
    }
    app.save_settings()?;
    app.regroup();
    app.apply(&[]).await?;
    if app.core.is_running() {
        core::close_all_connections(&app.api()).await;
    }
    Ok(state_view(&app))
}

#[tauri::command]
pub async fn delete_group(app: S<'_>, id: String) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    {
        let mut s = app.settings.lock().unwrap();
        let mut list = s.node_groups();
        list.retain(|g| g.id != id);
        s.groups = Some(list);
    }
    app.save_settings()?;
    app.regroup();
    app.apply(&[]).await?;
    Ok(state_view(&app))
}

/// 调整分组顺序：delta = -1 上移，1 下移
#[tauri::command]
pub async fn move_group(app: S<'_>, id: String, delta: i32) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    {
        let mut s = app.settings.lock().unwrap();
        let mut list = s.node_groups();
        if let Some(i) = list.iter().position(|g| g.id == id) {
            let j = i as i32 + delta;
            if j >= 0 && (j as usize) < list.len() {
                list.swap(i, j as usize);
            }
        }
        s.groups = Some(list);
    }
    app.save_settings()?;
    app.regroup();
    app.apply(&[]).await?;
    Ok(state_view(&app))
}

/// 恢复默认的“美国”“台湾”两个分组
#[tauri::command]
pub async fn reset_groups(app: S<'_>) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    app.settings.lock().unwrap().groups = Some(subscription::default_groups());
    app.save_settings()?;
    app.regroup();
    app.apply(&[]).await?;
    Ok(state_view(&app))
}

// ───────────────────────── 增强模式（TUN） ─────────────────────────

/// 打开或关闭增强模式。Mac 第一次打开会弹出系统密码框；
/// Windows 没有管理员权限时返回 "NEED_ADMIN"，界面再提示以管理员身份重新启动
#[tauri::command]
pub async fn set_tun(app: S<'_>, enabled: bool) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    if enabled && !tun::privileged(app.core.binary()) {
        match tun::platform() {
            "mac" => {
                let bin = app.core.binary().ok_or_else(|| tr!("找不到代理内核文件。", "Can't find the proxy core file."))?.to_path_buf();
                let stage = app.dir.join("mihomo-stage");
                tauri::async_runtime::spawn_blocking(move || tun::grant(&bin, &stage))
                    .await
                    .map_err(|e| e.to_string())??;
                // 授权后内核文件变成 root 所有，之前内核写下的文件要能继续用：内核目录保持不变即可
            }
            "windows" => {
                app.settings.lock().unwrap().tun_enabled = true;
                app.save_settings()?;
                return Err("NEED_ADMIN".into());
            }
            _ => return Err(tr!("这个系统暂不支持增强模式。", "Enhanced mode isn't supported on this system.")),
        }
    }
    app.settings.lock().unwrap().tun_enabled = enabled;
    app.save_settings()?;
    app.restart_core().await?;
    Ok(state_view(&app))
}

/// Windows：以管理员身份重新启动软件（增强模式需要）
#[tauri::command]
pub async fn relaunch_as_admin(app: S<'_>, handle: tauri::AppHandle) -> Result<(), String> {
    tun::relaunch_as_admin()?;
    // 新的（管理员）实例已启动：本实例退出，退出时会恢复系统代理，新实例启动后自动重新连接
    app.settings.lock().unwrap().tun_enabled = true;
    app.save_settings()?;
    handle.exit(0);
    Ok(())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub connected: bool,
    /// 默认地区组当前选的（地区名 / 节点名 / 自动选择）
    pub selected: String,
    /// 最终用的节点
    pub node: String,
    pub region: String,
    pub flag: String,
    pub delay: Option<u64>,
    /// 测了延迟但节点没响应（超时或连不上）
    pub delay_failed: bool,
    pub error: Option<String>,
    /// 科研版：稳定线路现在用的节点
    pub stable: String,
}

#[tauri::command]
pub async fn status(app: S<'_>, with_delay: bool) -> Result<Status, String> {
    let running = app.core.is_running();
    let mut st = Status {
        running,
        connected: running && app.connected.load(Ordering::SeqCst),
        error: app.core.last_error.lock().unwrap().clone(),
        ..Default::default()
    };
    if !running {
        return Ok(st);
    }
    let api = app.api();
    let Ok(all) = check::api_get(&api, "/proxies").await else {
        return Ok(st);
    };
    let pmap = &all["proxies"];
    st.selected = check::str_of(&pmap[GROUP_DEFAULT], "now");
    // 一路找到真正的节点
    let mut cur = st.selected.clone();
    for _ in 0..5 {
        let next = check::str_of(&pmap[&cur], "now");
        if next.is_empty() {
            break;
        }
        cur = next;
    }
    st.node = cur.clone();
    if crate::edition::is_research() {
        st.stable = check::str_of(&pmap[configgen::GROUP_STABLE], "now");
    }
    if cur == "DIRECT" {
        st.region = tr!("直连", "Direct");
    } else if let Some(r) = app.parsed.lock().unwrap().region_of(&cur) {
        st.region = r.name.clone();
        st.flag = r.flag.clone();
    }
    if with_delay && cur != "DIRECT" && !cur.is_empty() {
        let path = format!(
            "/proxies/{}/delay?timeout=5000&url={}",
            urlencoding::encode(&cur),
            urlencoding::encode("https://www.gstatic.com/generate_204")
        );
        st.delay = check::api_get(&api, &path).await.ok().and_then(|v| v["delay"].as_u64());
        st.delay_failed = st.delay.is_none();
    }
    Ok(st)
}

/// 测所有节点的延迟，返回 {节点名: 毫秒}，失败的不在里面
#[tauri::command]
pub async fn node_delays(app: S<'_>) -> Result<Value, String> {
    if !app.core.is_running() {
        return Err(tr!("请先连接。", "Connect first."));
    }
    core::group_delay(&app.api(), configgen::GROUP_FASTEST).await
}

#[tauri::command]
pub async fn check_url(app: S<'_>, url: String) -> Result<CheckResult, String> {
    if !app.core.is_running() {
        return Err(tr!("请先在首页点「连接」。", "Connect first on the Home page."));
    }
    check::inspect(app.api(), url, true).await
}

#[tauri::command]
pub fn core_log(app: S) -> String {
    app.core.log_tail()
}

#[tauri::command]
pub fn current_config(app: S) -> String {
    std::fs::read_to_string(app.core.config_path()).unwrap_or_default()
}

/// 导出手机配置：写到「下载」文件夹并在访达 / 资源管理器里选中它，返回文件路径
#[tauri::command]
pub fn export_mobile_config(app: S, handle: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let st = app.settings.lock().unwrap().clone();
    let parsed = app.parsed.lock().unwrap().clone();
    let texts: Vec<String> = st
        .subscriptions
        .iter()
        .map(|x| std::fs::read_to_string(sub_file(&app.dir, &x.id)).unwrap_or_default())
        .collect();
    let sites = sites::catalog(&st);
    let yaml = crate::mobile::generate(&crate::mobile::MobileInput {
        settings: &st,
        parsed: &parsed,
        sites: &sites,
        texts: &texts,
    })?;
    let dir = handle.path().download_dir().unwrap_or_else(|_| app.dir.clone());
    let app_name = crate::edition::name();
    let path = dir.join(tr!("{app_name} 手机配置.yaml", "{app_name} mobile config.yaml"));
    std::fs::write(&path, yaml).map_err(|e| tr!("无法保存文件：{e}", "Can't save the file: {e}"))?;
    reveal(&path);
    Ok(path.to_string_lossy().to_string())
}

/// 在访达 / 资源管理器里显示并选中文件
fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = path;
}

/// 改端口（高级）
#[tauri::command]
pub async fn set_ports(app: S<'_>, mixed_port: u16, controller_port: u16) -> Result<StateView, String> {
    let _g = app.op.lock().await;
    if mixed_port < 1024 || controller_port < 1024 || mixed_port == controller_port {
        return Err(tr!("端口需在 1024–65535 之间，且两个端口不能相同。", "Ports must be between 1024 and 65535, and the two ports must differ."));
    }
    let was_connected = app.connected.load(Ordering::SeqCst);
    let was_running = app.core.is_running();
    app.disconnect(false)?;
    {
        let mut s = app.settings.lock().unwrap();
        s.mixed_port = mixed_port;
        s.controller_port = controller_port;
    }
    app.save_settings()?;
    if was_connected {
        app.connect().await?;
    } else if was_running {
        app.start_core().await?;
    }
    Ok(state_view(&app))
}

/// 启动时：恢复上次崩溃留下的系统代理。不自动连接，由用户自己点小猫连接。
pub async fn on_startup(app: &AppState) {
    let port = app.settings.lock().unwrap().mixed_port;
    // 上次被强制结束时留下的内核（会让增强模式断网）
    app.core.kill_leftovers();
    sysproxy_ctl::recover_on_startup(&app.dir, port);
    // 上次没来得及撤销的终端代理（崩溃、强制退出）
    let _ = crate::termproxy::disable(&app.dir);
    // 刚自动更新完：更新前是连着的，就自动连上（平时打开软件不会自动连接）
    let resume = std::mem::take(&mut app.settings.lock().unwrap().resume_after_update);
    if resume {
        let _ = app.save_settings();
        let _g = app.op.lock().await;
        if let Err(e) = app.connect().await {
            *app.core.last_error.lock().unwrap() = Some(e);
        }
    }
}

#[cfg(test)]
mod e2e {
    //! 需要真实 mihomo（CFY_MIHOMO）、本地订阅服务（CFY_SUB_URL）。运行：cargo test -- --ignored
    use super::*;

    async fn live_rules(app: &AppState) -> Vec<String> {
        let api = app.api();
        let v: Value = reqwest::Client::new()
            .get(format!("http://{}/rules", api.controller))
            .bearer_auth(&api.secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| format!("{},{},{}", r["type"].as_str().unwrap(), r["payload"].as_str().unwrap(), r["proxy"].as_str().unwrap()))
            .collect()
    }

    /// 科研版：内核里有稳定线路；运行中打开校园网，出版社立即改成直连（只需要 CFY_MIHOMO、CFY_GEO）
    /// cargo test research_live -- --ignored --test-threads=1
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn research_live() {
        crate::edition::set_research_for_test(Some(true));
        let dir = std::env::temp_dir().join(format!("cfy-research-{}", settings::now_ms()));
        let geo = std::env::var("CFY_GEO").ok().map(PathBuf::from);
        let app = AppState::new(dir.clone(), geo);
        {
            let mut s = app.settings.lock().unwrap();
            assert_eq!((s.mixed_port, s.controller_port), (7900, 9099), "科研版默认端口");
            s.mixed_port = 18900;
            s.controller_port = 19199;
        }
        app.import_file_sub(
            "t.yaml",
            "proxies:\n  - {name: \"美国 01\", type: socks5, server: 127.0.0.1, port: 1080}\n  - {name: \"美国 02\", type: socks5, server: 127.0.0.1, port: 1081}\n",
        )
        .unwrap();
        app.reparse();
        app.start_core().await.unwrap();
        let api = app.api();
        let proxies = check::api_get(&api, "/proxies").await.unwrap();
        assert_eq!(proxies["proxies"][configgen::GROUP_STABLE]["type"].as_str(), Some("URLTest"));
        let rules = live_rules(&app).await;
        assert!(rules.iter().any(|r| r == "DomainSuffix,scholar.google.com,稳定线路"), "{rules:?}");
        assert!(!rules.iter().any(|r| r.contains("sciencedirect.com")));

        app.settings.lock().unwrap().campus_mode = true;
        app.apply(&[]).await.unwrap();
        let rules = live_rules(&app).await;
        assert!(rules.iter().any(|r| r == "DomainSuffix,sciencedirect.com,DIRECT"));
        assert!(rules.iter().any(|r| r == "DomainSuffix,scholar.google.com,稳定线路"));
        println!("校园网开启后内核规则（前 12 条）：{:#?}", &rules[..12.min(rules.len())]);
        app.core.stop();
        let _ = std::fs::remove_dir_all(dir);
        crate::edition::set_research_for_test(None);
    }

    /// 运行中切换全局 / 规则，内核实际加载的规则跟着变（只需要 CFY_MIHOMO）
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn global_switch_live() {
        let dir = std::env::temp_dir().join(format!("cfy-global-{}", settings::now_ms()));
        let app = AppState::new(dir.clone(), None);
        {
            let mut s = app.settings.lock().unwrap();
            s.mixed_port = 17999;
            s.controller_port = 19198;
            s.custom_rules.push(CustomRule { kind: "keyword".into(), value: "openai".into(), target: "DIRECT".into() });
        }
        app.import_file_sub("t.yaml", "proxies:\n  - {name: \"日本 01\", type: socks5, server: 127.0.0.1, port: 1080}\n")
            .unwrap();
        app.reparse();
        app.start_core().await.unwrap();

        let rules = live_rules(&app).await;
        assert!(rules.iter().any(|r| r.contains("openai")), "{rules:?}");

        app.settings.lock().unwrap().global_mode = true;
        app.apply(&[]).await.unwrap();
        let rules = live_rules(&app).await;
        println!("全局模式下内核的规则：{rules:#?}");
        assert!(!rules.iter().any(|r| r.contains("openai")));
        assert!(rules.last().unwrap().starts_with("Match"));
        assert!(rules.last().unwrap().ends_with(GROUP_DEFAULT));

        app.settings.lock().unwrap().global_mode = false;
        app.apply(&[]).await.unwrap();
        assert!(live_rules(&app).await.iter().any(|r| r.contains("openai")));
        app.core.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn full_flow() {
        let sub = std::env::var("CFY_SUB_URL").expect("CFY_SUB_URL");
        let dir = std::env::temp_dir().join(format!("cfy-e2e-{}", settings::now_ms()));
        let geo = std::env::var("CFY_GEO").ok().map(PathBuf::from);
        let app = AppState::new(dir.clone(), geo);
        {
            let mut s = app.settings.lock().unwrap();
            s.mixed_port = 17899;
            s.controller_port = 19098;
            let mut g = subscription::default_groups();
            for (id, name, kw) in [("jp", "日本", "日本"), ("sg", "新加坡", "SG"), ("hk", "香港", "香港")] {
                g.push(NodeGroup { id: id.into(), name: name.into(), keywords: vec![kw.into()], nodes: vec![] });
            }
            s.groups = Some(g);
        }
        app.fetch_url_sub(&sub).await.unwrap();
        app.reparse();
        let p = app.parsed.lock().unwrap().clone();
        let names: Vec<_> = p.regions.iter().map(|r| r.name.clone()).collect();
        println!("regions: {names:?}");
        assert_eq!(names, vec!["美国", "日本", "新加坡", "香港", "其他"]);
        assert_eq!(app.settings.lock().unwrap().subscriptions[0].usage.as_ref().unwrap().total, 107374182400);

        app.start_core().await.expect("core start");
        assert!(app.core.is_running());
        assert!(app.geo_ready(), "geo files should be copied");

        // 网站选择：Discord 走日本
        app.settings.lock().unwrap().site_choices.insert("discord".into(), "region:日本".into());
        app.apply(&["discord.com".into()]).await.expect("reload");
        let r = check::inspect(app.api(), "http://discord.com:18081".into(), false).await.expect("check");
        println!("discord: rule={} {} chains={:?}", r.rule, r.rule_payload, r.chains);
        assert_eq!(r.rule_payload, "discord.com");
        assert!(r.chains.contains(&"日本".to_string()));
        assert_eq!(r.node, "🇯🇵 日本 东京");

        // 具体节点
        app.settings.lock().unwrap().site_choices.insert("discord".into(), "node:US 洛杉矶 01".into());
        app.apply(&["discord.com".into()]).await.unwrap();
        let r = check::inspect(app.api(), "http://discord.com:18081".into(), false).await.unwrap();
        assert_eq!(r.node, "US 洛杉矶 01");

        // 自定义规则优先于网站选择
        app.settings.lock().unwrap().custom_rules =
            vec![CustomRule { kind: "suffix".into(), value: "discord.com".into(), target: "region:新加坡".into() }];
        app.apply(&["discord.com".into()]).await.unwrap();
        let r = check::inspect(app.api(), "http://discord.com:18081".into(), false).await.unwrap();
        assert_eq!(r.node, "SG-Premium");

        // 国内直连（GEOSITE）：规则已加载
        assert!(app.core.log_tail().contains("GeoSite rule cn"));
        assert!(std::fs::read_to_string(app.core.config_path()).unwrap().contains("GEOSITE,cn,DIRECT"));

        // 默认地区切到香港
        app.settings.lock().unwrap().default_target = "region:香港".into();
        app.settings.lock().unwrap().custom_rules.push(CustomRule {
            kind: "keyword".into(),
            value: "example".into(),
            target: "default".into(),
        });
        app.apply(&[]).await.unwrap();
        core::select(&app.api(), GROUP_DEFAULT, "香港").await.unwrap();
        let r = check::inspect(app.api(), "http://example.org:18081".into(), false).await.unwrap();
        println!("example: rule={} chains={:?}", r.rule, r.chains);
        assert!(r.chains.contains(&"香港".to_string()));

        // 多订阅：从文件再导入一份，节点合并；删掉后恢复
        let before = app.parsed.lock().unwrap().proxies.len();
        app.import_file_sub("备用.yaml", "proxies:\n  - {name: \"台湾 备用\", type: ss, server: 127.0.0.1, port: 18388, cipher: aes-128-gcm, password: testpass123}\n").unwrap();
        app.after_subs_changed().await.expect("reload with two subs");
        assert_eq!(app.parsed.lock().unwrap().proxies.len(), before + 1);
        assert!(app.parsed.lock().unwrap().regions.iter().any(|r| r.name == "台湾"));
        let subs = app.settings.lock().unwrap().subscriptions.clone();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1].name, "备用");
        app.settings.lock().unwrap().subscriptions.truncate(1);
        app.after_subs_changed().await.unwrap();
        assert_eq!(app.parsed.lock().unwrap().proxies.len(), before);

        // 内核被杀后自动重启
        let pid_before = app.core.is_running();
        assert!(pid_before);
        std::process::Command::new("pkill").args(["-f", &dir.to_string_lossy()]).status().unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(app.core.is_running(), "should auto restart");

        app.core.stop();
        assert!(!app.core.is_running());
    }
}
