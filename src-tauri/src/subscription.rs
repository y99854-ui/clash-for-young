//! 订阅：下载、解析 Clash 格式，并按“分组”归类节点。
//! 分组 = 名字 + 关键词 + 手动挑选的节点。默认只有“美国”“台湾”两个，用户可以增删改。

use crate::settings::{NodeGroup, Usage};
use regex::Regex;
use serde::Serialize;
use serde_yaml::{Mapping, Value};
use std::sync::OnceLock;
use std::time::Duration;

pub const OTHER_REGION: &str = "其他";

/// 默认的两个分组
pub fn default_groups() -> Vec<NodeGroup> {
    let kw = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    vec![
        NodeGroup {
            id: "us".into(),
            name: "美国".into(),
            keywords: kw(&[
                "🇺🇸", "美国", "美國", "洛杉矶", "洛杉磯", "硅谷", "圣何塞", "聖何塞", "西雅图", "纽约", "紐約", "芝加哥",
                "达拉斯", "凤凰城", "US", "USA", "United States", "America", "Los Angeles", "San Jose", "Seattle",
            ]),
            nodes: vec![],
        },
        NodeGroup {
            id: "tw".into(),
            name: "台湾".into(),
            keywords: kw(&["🇹🇼", "台湾", "台灣", "台北", "新北", "彰化", "TW", "Taiwan", "Taipei"]),
            nodes: vec![],
        },
    ]
}

/// 关键词是否命中节点名：不分大小写；3 个字母以内的纯英文缩写（如 US、TW）要求前后不是字母，
/// 避免 “USDT”“TWO” 之类误判
pub fn keyword_hits(keyword: &str, name: &str) -> bool {
    let k = keyword.trim();
    if k.is_empty() {
        return false;
    }
    let kl = k.to_lowercase();
    let nl = name.to_lowercase();
    if k.len() <= 3 && k.chars().all(|c| c.is_ascii_alphabetic()) {
        let chars: Vec<char> = nl.chars().collect();
        let kc: Vec<char> = kl.chars().collect();
        for i in 0..chars.len() {
            if i + kc.len() > chars.len() {
                break;
            }
            if chars[i..i + kc.len()] == kc[..] {
                let before_ok = i == 0 || !chars[i - 1].is_ascii_alphabetic();
                let after_ok = i + kc.len() == chars.len() || !chars[i + kc.len()].is_ascii_alphabetic();
                if before_ok && after_ok {
                    return true;
                }
            }
        }
        false
    } else {
        nl.contains(&kl)
    }
}

pub fn group_matches(g: &NodeGroup, name: &str) -> bool {
    g.nodes.iter().any(|n| n == name) || g.keywords.iter().any(|k| keyword_hits(k, name))
}

/// 分组的图标：关键词里有国旗就用国旗
pub fn flag_of_group(g: &NodeGroup) -> String {
    g.keywords
        .iter()
        .find(|k| {
            let cs: Vec<char> = k.chars().collect();
            cs.len() == 2 && cs.iter().all(|c| ('\u{1F1E6}'..='\u{1F1FF}').contains(c))
        })
        .cloned()
        .unwrap_or_else(|| "📁".into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    /// 分组 id；“其他”为空
    pub id: String,
    pub name: String,
    pub flag: String,
    pub nodes: Vec<String>,
}

/// 解析好的订阅
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// 原样保留的节点配置（名字已去重）
    pub proxies: Vec<Mapping>,
    /// 有节点的分组（按用户的顺序），没进任何分组的节点放在最后的“其他”
    pub regions: Vec<Region>,
    /// 全部分组（包括没有节点的），给界面管理用
    pub all_groups: Vec<Region>,
}

impl Parsed {
    pub fn node_names(&self) -> Vec<String> {
        self.proxies.iter().filter_map(|p| p.get("name").and_then(|v| v.as_str()).map(String::from)).collect()
    }

    /// 按分组定义重新归类节点
    pub fn apply_groups(&mut self, defs: &[NodeGroup]) {
        let names = self.node_names();
        let mut all = vec![];
        let mut used = std::collections::HashSet::new();
        for g in defs {
            let nodes: Vec<String> = names.iter().filter(|n| group_matches(g, n)).cloned().collect();
            for n in &nodes {
                used.insert(n.clone());
            }
            all.push(Region { id: g.id.clone(), name: g.name.clone(), flag: flag_of_group(g), nodes });
        }
        let other: Vec<String> = names.into_iter().filter(|n| !used.contains(n)).collect();
        self.regions = all.iter().filter(|r| !r.nodes.is_empty()).cloned().collect();
        if !other.is_empty() {
            self.regions.push(Region { id: String::new(), name: OTHER_REGION.into(), flag: "🌐".into(), nodes: other });
        }
        self.all_groups = all;
    }

    /// 节点所在的第一个分组（首页显示用）
    pub fn region_of(&self, node: &str) -> Option<&Region> {
        self.regions.iter().find(|r| r.nodes.iter().any(|n| n == node))
    }
}

/// 这些名字被软件生成的策略组或内核占用
pub fn reserved_names() -> Vec<String> {
    let mut v: Vec<String> = vec![OTHER_REGION.into()];
    v.push(crate::configgen::GROUP_DEFAULT.into());
    v.push(crate::configgen::GROUP_FASTEST.into());
    v.extend(["DIRECT", "REJECT", "REJECT-DROP", "PASS", "COMPATIBLE", "GLOBAL"].iter().map(|s| s.to_string()));
    v
}

/// 订阅里常见的“信息节点”（剩余流量、到期时间、官网），不是真节点
const INFO_PATTERN: &str =
    r"(?i)剩余流量|套餐到期|到期时间|过期时间|距离下次重置|下次重置|官网|官方网站|最新网址|expire|traffic|剩余\s*[:：]";

fn info_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(INFO_PATTERN).unwrap())
}

/// defs：分组定义；节点不能和分组重名
pub fn parse(text: &str, defs: &[NodeGroup]) -> Result<Parsed, String> {
    let doc: Value = serde_yaml::from_str(text).map_err(|_| not_clash_hint(text))?;
    let list = doc
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .ok_or_else(|| not_clash_hint(text))?;

    let mut reserved = reserved_names();
    reserved.extend(defs.iter().map(|g| g.name.clone()));
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut proxies = vec![];
    for item in list {
        let Some(map) = item.as_mapping() else { continue };
        let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let has_server = map.get("server").is_some() || map.get("type").and_then(|v| v.as_str()) == Some("wireguard");
        if name.is_empty() || map.get("type").is_none() || !has_server {
            continue;
        }
        if info_regex().is_match(&name) {
            continue;
        }
        // 去重：和生成的组名或其他节点重名时加序号
        let mut final_name = name.clone();
        let mut i = 2;
        while used.contains(&final_name) || reserved.contains(&final_name) {
            final_name = format!("{name} {i}");
            i += 1;
        }
        used.insert(final_name.clone());
        let mut m = map.clone();
        m.insert(Value::from("name"), Value::from(final_name));
        proxies.push(m);
    }
    if proxies.is_empty() {
        return Err("订阅里没有可用的节点。请确认链接正确、套餐没有过期。".into());
    }

    let mut parsed = Parsed { proxies, ..Default::default() };
    parsed.apply_groups(defs);
    Ok(parsed)
}

fn not_clash_hint(text: &str) -> String {
    let t = text.trim();
    if t.starts_with('<') {
        return "订阅链接返回的是网页而不是订阅内容。请检查链接是否完整。".into();
    }
    let looks_b64 = !t.is_empty() && t.len() > 20 && t.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-\r\n".contains(c));
    if looks_b64 || t.contains("://") {
        return "这不是 Clash 格式的订阅。请在机场后台复制「Clash」或「Clash Meta / mihomo」订阅链接。".into();
    }
    "无法识别订阅内容，请确认复制的是 Clash 订阅链接。".into()
}

/// 解析响应头 subscription-userinfo: upload=1; download=2; total=3; expire=4
pub fn parse_usage(header: &str) -> Option<Usage> {
    let mut u = Usage::default();
    let mut any = false;
    for part in header.split(';') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next().unwrap_or("").trim().to_ascii_lowercase();
        let v = kv.next().unwrap_or("").trim();
        let num = v.parse::<f64>().unwrap_or(0.0);
        match k.as_str() {
            "upload" => u.upload = num as u64,
            "download" => u.download = num as u64,
            "total" => u.total = num as u64,
            "expire" => u.expire = num as i64,
            _ => continue,
        }
        any = true;
    }
    any.then_some(u)
}

pub struct Fetched {
    pub text: String,
    pub usage: Option<Usage>,
}

/// 下载订阅。先直连；失败且给了本软件的代理端口时，再经代理试一次。
pub async fn fetch(url: &str, via_proxy_port: Option<u16>) -> Result<Fetched, String> {
    let url = url.trim();
    let parsed = url::Url::parse(url).map_err(|_| "订阅链接格式不对，应以 http:// 或 https:// 开头。".to_string())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("订阅链接应以 http:// 或 https:// 开头。".into());
    }
    match fetch_once(url, None).await {
        Ok(f) => Ok(f),
        Err(e) => match via_proxy_port {
            Some(port) => fetch_once(url, Some(port)).await.map_err(|_| e),
            None => Err(e),
        },
    }
}

async fn fetch_once(url: &str, proxy_port: Option<u16>) -> Result<Fetched, String> {
    let mut b = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("clash.meta/1.19 mihomo ClashForYoung/0.1");
    b = match proxy_port {
        Some(p) => b.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?),
        None => b.no_proxy(),
    };
    let client = b.build().map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| format!("下载订阅失败：{}", short_err(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("下载订阅失败：服务器返回 {}。链接可能已失效，请到机场后台重新复制。", status.as_u16()));
    }
    let usage = resp
        .headers()
        .get("subscription-userinfo")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_usage);
    let text = resp.text().await.map_err(|e| format!("下载订阅失败：{}", short_err(&e)))?;
    Ok(Fetched { text, usage })
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "连接超时".into()
    } else if e.is_connect() {
        "连不上订阅服务器".into()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_groups_match() {
        let d = default_groups();
        let us = &d[0];
        let tw = &d[1];
        assert!(group_matches(us, "🇺🇸 美国 洛杉矶 01"));
        assert!(group_matches(us, "US-Seattle-02"));
        assert!(group_matches(us, "us 03"));
        assert!(!group_matches(us, "USDT 充值"));
        assert!(!group_matches(us, "Russia 01"));
        assert!(group_matches(tw, "台湾家宽"));
        assert!(group_matches(tw, "TW|HiNet"));
        assert!(!group_matches(tw, "TWO 节点"));
        assert_eq!(flag_of_group(us), "🇺🇸");
    }

    #[test]
    fn custom_group_and_other() {
        let mut defs = default_groups();
        defs.push(NodeGroup {
            id: "g1".into(),
            name: "日本".into(),
            keywords: vec!["日本".into(), "JP".into()],
            nodes: vec!["Mars".into()],
        });
        let y = r#"
proxies:
  - {name: "剩余流量：100GB", type: ss, server: a.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "香港 01", type: ss, server: a.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "香港 01", type: ss, server: b.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "日本", type: ss, server: c.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "JP-02", type: ss, server: c.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "Mars", type: ss, server: d.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "美国 01", type: ss, server: d.com, port: 1, cipher: aes-128-gcm, password: x}
"#;
        let p = parse(y, &defs).unwrap();
        assert_eq!(p.node_names(), vec!["香港 01", "香港 01 2", "日本 2", "JP-02", "Mars", "美国 01"]);
        let names: Vec<_> = p.regions.iter().map(|r| r.name.as_str()).collect();
        // 台湾没有节点，不出现在可选分组里，但仍在 all_groups 里
        assert_eq!(names, vec!["美国", "日本", OTHER_REGION]);
        assert_eq!(p.all_groups.len(), 3);
        assert_eq!(p.regions[1].nodes, vec!["日本 2", "JP-02", "Mars"]);
        assert_eq!(p.regions[2].nodes, vec!["香港 01", "香港 01 2"]);
    }

    #[test]
    fn not_clash_messages() {
        assert!(parse("dm1lc3M6Ly9leGFtcGxlLmNvbTo0NDM/aGVsbG8=", &[]).unwrap_err().contains("Clash"));
        assert!(parse("<html></html>", &[]).unwrap_err().contains("网页"));
    }

    #[test]
    fn usage_header() {
        let u = parse_usage("upload=100; download=200; total=1000; expire=1700000000").unwrap();
        assert_eq!(u.total, 1000);
        assert_eq!(u.expire, 1700000000);
    }
}
