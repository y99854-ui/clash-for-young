//! 用户设置：订阅、默认走向、网站选择、自定义网站和规则。保存在数据目录的 settings.json。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 走向（target）的写法：
/// - ""                 跟随默认（网站页的“自动”）
/// - "DIRECT"           直连
/// - "REJECT"           拦截（只在自定义规则里出现）
/// - "fastest"          全部节点里最快的
/// - "default"          首页选的默认地区
/// - "region:美国"      某个分组（组内自动选最快）
/// - "node:节点名"      某个具体节点
pub type Target = String;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Subscription {
    pub url: String,
    /// 上次成功更新的时间（毫秒时间戳）
    pub updated_at: i64,
    /// 订阅响应头 subscription-userinfo
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Usage {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    /// 到期时间（秒时间戳），0 表示未知
    pub expire: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomSite {
    pub id: String,
    pub name: String,
    pub url: String,
    pub domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomRule {
    /// suffix（域名及子域名）/ domain（完整域名）/ keyword（域名关键词）/ ipcidr（IP 段）
    pub kind: String,
    pub value: String,
    pub target: Target,
}

/// 节点分组：名字 + 关键词（节点名包含任一关键词即归入）+ 手动挑选的节点
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct NodeGroup {
    pub id: String,
    pub name: String,
    pub keywords: Vec<String>,
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// 第一版只用第一个，结构上保留多个
    pub subscriptions: Vec<Subscription>,
    /// 首页“默认地区”：fastest / region:xx / node:xx / DIRECT
    pub default_target: Target,
    /// 网站 id → 走向
    pub site_choices: BTreeMap<String, Target>,
    /// 内置网站被用户改过的域名
    pub site_domains: BTreeMap<String, Vec<String>>,
    pub custom_sites: Vec<CustomSite>,
    pub custom_rules: Vec<CustomRule>,
    /// 节点分组；None 表示从没改过，用默认的“美国”“台湾”
    pub groups: Option<Vec<NodeGroup>>,
    pub mixed_port: u16,
    pub controller_port: u16,
    pub secret: String,
    /// 上次退出时是否处于连接状态（下次打开自动连接）
    pub was_connected: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            subscriptions: vec![],
            default_target: "fastest".into(),
            site_choices: BTreeMap::new(),
            site_domains: BTreeMap::new(),
            custom_sites: vec![],
            custom_rules: vec![],
            groups: None,
            mixed_port: 7899,
            controller_port: 9098,
            secret: String::new(),
            was_connected: false,
        }
    }
}

impl Settings {
    pub fn node_groups(&self) -> Vec<NodeGroup> {
        self.groups.clone().unwrap_or_else(crate::subscription::default_groups)
    }
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

pub fn load(dir: &Path) -> Settings {
    let mut s: Settings = std::fs::read_to_string(settings_path(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if s.secret.is_empty() {
        s.secret = random_secret();
    }
    if s.default_target.is_empty() {
        s.default_target = "fastest".into();
    }
    s
}

pub fn save(dir: &Path, s: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("无法创建数据目录：{e}"))?;
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let tmp = dir.join("settings.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("无法保存设置：{e}"))?;
    std::fs::rename(&tmp, settings_path(dir)).map_err(|e| format!("无法保存设置：{e}"))
}

fn random_secret() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let addr = &n as *const _ as usize as u128;
    let mut x = n ^ (addr << 64) ^ (std::process::id() as u128) << 32;
    let mut out = String::new();
    for _ in 0..24 {
        // xorshift
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let c = (x % 36) as u8;
        out.push(if c < 10 { (b'0' + c) as char } else { (b'a' + c - 10) as char });
    }
    out
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
