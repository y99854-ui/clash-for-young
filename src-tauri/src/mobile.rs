//! 导出手机配置：把电脑上的分组、网站选择、自定义规则做成一份 Clash（mihomo）配置，
//! 导入 Clash Mi（iPhone）、Clash Meta for Android、FlClash 等手机软件使用。
//!
//! - 链接订阅写成 proxy-providers：手机自己下载、自动更新节点，分组用正则按节点名筛选
//! - 从文件导入的订阅没有链接，节点直接写进配置
//! - 规则和电脑上完全一样（直接取电脑配置生成的规则）

use crate::configgen::{self, GenInput, GROUP_DEFAULT, GROUP_FASTEST};
use crate::settings::{NodeGroup, Settings};
use crate::sites::Site;
use crate::subscription::{self, Parsed, Tagged, OTHER_REGION};
use serde_yaml::{Mapping, Value};
use std::collections::{HashMap, HashSet};

const TEST_URL: &str = "https://www.gstatic.com/generate_204";
/// 指向单个节点的规则在手机上要包一层组（订阅里的节点不能直接写在规则里）
const NODE_PREFIX: &str = "节点 · ";

pub struct MobileInput<'a> {
    pub settings: &'a Settings,
    /// 电脑上合并好的节点和分组
    pub parsed: &'a Parsed,
    pub sites: &'a [Site],
    /// 和 settings.subscriptions 一一对应的订阅内容（读不到的是空字符串）
    pub texts: &'a [String],
}

fn s(v: &str) -> Value {
    Value::from(v)
}

fn map(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Mapping::new();
    for (k, v) in pairs {
        m.insert(s(k), v);
    }
    Value::Mapping(m)
}

fn seq(items: Vec<String>) -> Value {
    Value::Sequence(items.into_iter().map(Value::from).collect())
}

fn provider_name(sub_id: &str) -> String {
    format!("订阅-{sub_id}")
}

pub fn generate(input: &MobileInput) -> Result<String, String> {
    let st = input.settings;
    let parsed = input.parsed;
    if parsed.proxies.is_empty() {
        return Err("还没有可用的节点，先添加订阅。".into());
    }
    let defs: Vec<NodeGroup> = st.node_groups();

    // 电脑配置（增强模式那一版：fake-ip、接管 IPv6），从里面拿规则和 DNS
    let desk = configgen::generate(&GenInput {
        settings: st,
        parsed,
        sites: input.sites,
        geo_ready: true,
        tun: true,
    });
    let desk: Value = serde_yaml::from_str(&desk.yaml).map_err(|e| e.to_string())?;

    // ── 节点来源 ──
    let tagged: Vec<Tagged> = subscription::tagged_proxies(input.texts, &defs);
    let is_url_sub = |i: usize| st.subscriptions.get(i).map(|x| !x.url.trim().is_empty()).unwrap_or(false);
    let mut providers = Mapping::new();
    let mut provider_names: Vec<String> = vec![];
    for (i, sub) in st.subscriptions.iter().enumerate() {
        if !is_url_sub(i) || !tagged.iter().any(|t| t.source == i) {
            continue;
        }
        let name = provider_name(&sub.id);
        providers.insert(
            s(&name),
            map(vec![
                ("type", s("http")),
                ("url", s(sub.url.trim())),
                ("interval", Value::from(21600)),
                ("path", Value::from(format!("./providers/cfy-{}.yaml", sub.id))),
                ("override", map(vec![("udp", Value::from(true))])),
            ]),
        );
        provider_names.push(name);
    }
    // 文件导入的节点直接写进配置
    let inline: Vec<&Tagged> = tagged.iter().filter(|t| !is_url_sub(t.source)).collect();
    let inline_names: HashSet<String> = inline.iter().map(|t| t.name()).collect();
    let proxies: Vec<Value> = inline
        .iter()
        .map(|t| {
            let mut p = t.proxy.clone();
            p.insert(s("udp"), Value::from(true));
            Value::Mapping(p)
        })
        .collect();
    // 链接订阅里的节点：电脑上的名字 → 订阅里的原名
    let provider_nodes: HashMap<String, String> =
        tagged.iter().filter(|t| is_url_sub(t.source)).map(|t| (t.name(), t.original.clone())).collect();

    let info = subscription::info_pattern().to_string();
    let exclude_info = |extra: Option<&str>| -> String {
        match extra {
            Some(x) => format!("{info}|{x}"),
            None => info.clone(),
        }
    };

    // 某个分组里，文件导入的节点（按电脑上的分组结果）
    let inline_of = |region: &str| -> Vec<String> {
        parsed
            .regions
            .iter()
            .find(|r| r.name == region)
            .map(|r| r.nodes.iter().filter(|n| inline_names.contains(*n)).cloned().collect())
            .unwrap_or_default()
    };

    let url_test = |name: &str, nodes: Vec<String>, filter: Option<String>, exclude: String| -> Value {
        let mut pairs = vec![("name", s(name)), ("type", s("url-test"))];
        if !nodes.is_empty() {
            pairs.push(("proxies", seq(nodes)));
        }
        if !provider_names.is_empty() {
            pairs.push(("use", seq(provider_names.clone())));
        }
        if let Some(f) = filter {
            pairs.push(("filter", s(&f)));
        }
        pairs.push(("exclude-filter", s(&exclude)));
        pairs.extend([
            ("url", s(TEST_URL)),
            ("interval", Value::from(300)),
            ("tolerance", Value::from(50)),
            ("lazy", Value::from(true)),
        ]);
        map(pairs)
    };

    // ── 策略组 ──
    let region_names: Vec<String> = parsed.regions.iter().map(|r| r.name.clone()).collect();
    let mut groups: Vec<Value> = vec![];

    // 默认地区：顺序和电脑一样（选中的在第一个），后面再列出所有节点
    let desk_default: Vec<String> = desk["proxy-groups"]
        .as_sequence()
        .and_then(|g| g.iter().find(|x| x["name"].as_str() == Some(GROUP_DEFAULT)))
        .and_then(|g| g["proxies"].as_sequence())
        .map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mut default_members: Vec<String> = vec![];
    for m in desk_default {
        let keep = m == GROUP_FASTEST || m == "DIRECT" || region_names.contains(&m) || inline_names.contains(&m);
        if keep {
            default_members.push(m);
        } else if provider_nodes.contains_key(&m) && default_members.is_empty() {
            // 默认选了某个订阅节点：用包装组
            default_members.push(format!("{NODE_PREFIX}{m}"));
        }
    }
    let mut dpairs = vec![("name", s(GROUP_DEFAULT)), ("type", s("select")), ("proxies", seq(default_members))];
    if !provider_names.is_empty() {
        dpairs.push(("use", seq(provider_names.clone())));
        dpairs.push(("exclude-filter", s(&info)));
    }
    groups.push(map(dpairs));

    // 自动选择：全部节点
    groups.push(url_test(GROUP_FASTEST, inline.iter().map(|t| t.name()).collect(), None, info.clone()));

    // 各分组
    let all_regex: Vec<String> = defs.iter().filter_map(subscription::group_regex).collect();
    for r in &parsed.regions {
        if r.name == OTHER_REGION {
            // 其他：不属于任何分组的节点
            let ex = if all_regex.is_empty() { None } else { Some(all_regex.join("|")) };
            groups.push(url_test(&r.name, inline_of(&r.name), None, exclude_info(ex.as_deref())));
            continue;
        }
        let def = defs.iter().find(|d| d.name == r.name);
        let filter = def.and_then(subscription::group_regex);
        // 没有筛选条件的分组不能把整个订阅都放进去
        let use_providers = filter.is_some();
        let mut g = url_test(&r.name, inline_of(&r.name), filter, info.clone());
        if !use_providers {
            if let Value::Mapping(m) = &mut g {
                m.remove(s("use"));
            }
        }
        groups.push(g);
    }

    // ── 规则：和电脑一样；指向订阅节点的改成包装组 ──
    let mut node_groups: Vec<String> = vec![];
    let mut rules: Vec<String> = vec![];
    for r in desk["rules"].as_sequence().into_iter().flatten().filter_map(|x| x.as_str()) {
        let mut parts: Vec<String> = r.split(',').map(String::from).collect();
        let ti = if parts.last().map(|x| x == "no-resolve").unwrap_or(false) { parts.len() - 2 } else { parts.len() - 1 };
        if !r.starts_with("AND,") {
            if let Some(t) = parts.get(ti).cloned() {
                if provider_nodes.contains_key(&t) {
                    if !node_groups.contains(&t) {
                        node_groups.push(t.clone());
                    }
                    parts[ti] = format!("{NODE_PREFIX}{t}");
                }
            }
        }
        rules.push(parts.join(","));
    }
    // 默认地区里引用的节点也要有包装组
    if let Some(first) = groups[0]["proxies"].as_sequence().and_then(|v| v.first()).and_then(|v| v.as_str()) {
        if let Some(n) = first.strip_prefix(NODE_PREFIX) {
            if !node_groups.contains(&n.to_string()) {
                node_groups.push(n.to_string());
            }
        }
    }
    for n in node_groups {
        let orig = provider_nodes.get(&n).cloned().unwrap_or(n.clone());
        groups.push(map(vec![
            ("name", Value::from(format!("{NODE_PREFIX}{n}"))),
            ("type", s("select")),
            ("use", seq(provider_names.clone())),
            ("filter", Value::from(format!("^{}$", regex::escape(&orig)))),
        ]));
    }

    // ── 整体 ──
    let mut root = Mapping::new();
    for (k, v) in [
        ("mode", s("rule")),
        ("log-level", s("warning")),
        ("ipv6", Value::from(true)),
        ("unified-delay", Value::from(true)),
        ("tcp-concurrent", Value::from(true)),
        ("find-process-mode", s("off")),
        ("geo-auto-update", Value::from(true)),
        ("geo-update-interval", Value::from(168)),
    ] {
        root.insert(s(k), v);
    }
    root.insert(s("geox-url"), desk["geox-url"].clone());
    root.insert(s("profile"), map(vec![("store-selected", Value::from(true)), ("store-fake-ip", Value::from(true))]));
    root.insert(s("dns"), desk["dns"].clone());
    if !providers.is_empty() {
        root.insert(s("proxy-providers"), Value::Mapping(providers));
    }
    root.insert(s("proxies"), Value::Sequence(proxies));
    root.insert(s("proxy-groups"), Value::Sequence(groups));
    root.insert(s("rules"), seq(rules));
    Ok(format!(
        "# Clash for Young 导出的手机配置。里面有你的订阅链接，请不要发给别人。\n\
         # 适用：Clash Mi（iPhone）、Clash Meta for Android、FlClash 等 mihomo 内核的软件。\n{}",
        serde_yaml::to_string(&Value::Mapping(root)).map_err(|e| e.to_string())?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CustomRule, Subscription};

    const SUB_A: &str = r#"
proxies:
  - {name: "🇺🇸 美国 01", type: ss, server: a.com, port: 1, cipher: aes-128-gcm, password: x, udp: false}
  - {name: "台湾 TW 01", type: ss, server: b.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "Japan 01", type: ss, server: c.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "剩余流量：100G", type: ss, server: d.com, port: 1, cipher: aes-128-gcm, password: x}
"#;
    const SUB_FILE: &str = r#"
proxies:
  - {name: "US 家宽", type: ss, server: e.com, port: 1, cipher: aes-128-gcm, password: y}
  - {name: "Mars", type: ss, server: f.com, port: 1, cipher: aes-128-gcm, password: y}
"#;

    #[test]
    fn mobile_config() {
        let mut st = Settings::default();
        st.subscriptions = vec![
            Subscription { id: "s0".into(), name: "a".into(), url: "https://sub.example/abc".into(), ..Default::default() },
            Subscription { id: "s1".into(), name: "file".into(), url: String::new(), ..Default::default() },
        ];
        st.site_choices.insert("discord".into(), "node:Japan 01".into());
        st.custom_rules.push(CustomRule { kind: "suffix".into(), value: "example.com".into(), target: "region:台湾".into() });
        let texts = vec![SUB_A.to_string(), SUB_FILE.to_string()];
        let parsed = subscription::parse_many(&texts, &st.node_groups());
        let sites = crate::sites::catalog(&st);
        let yaml = generate(&MobileInput { settings: &st, parsed: &parsed, sites: &sites, texts: &texts }).unwrap();
        if let Ok(p) = std::env::var("CFY_DUMP_MOBILE") {
            std::fs::write(p, &yaml).unwrap();
        }
        let v: Value = serde_yaml::from_str(&yaml).unwrap();
        // 链接订阅变成 provider，文件订阅的节点写在 proxies
        assert_eq!(v["proxy-providers"]["订阅-s0"]["url"].as_str(), Some("https://sub.example/abc"));
        let names: Vec<&str> = v["proxies"].as_sequence().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["US 家宽", "Mars"]);
        // 没有电脑专用的设置
        assert!(v.get("mixed-port").is_none() && v.get("external-controller").is_none() && v.get("tun").is_none());
        let groups = v["proxy-groups"].as_sequence().unwrap();
        let g = |n: &str| groups.iter().find(|x| x["name"].as_str() == Some(n)).unwrap().clone();
        let us = g("美国");
        assert!(us["filter"].as_str().unwrap().contains("美国"));
        assert_eq!(us["proxies"][0].as_str(), Some("US 家宽"));
        assert!(g("其他")["exclude-filter"].as_str().unwrap().contains("美国"));
        // 指向单个订阅节点的规则包了一层组
        let rules: Vec<&str> = v["rules"].as_sequence().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
        assert!(rules.contains(&"DOMAIN-SUFFIX,discord.com,节点 · Japan 01"));
        assert!(rules.contains(&"DOMAIN-SUFFIX,example.com,台湾"));
        assert_eq!(g("节点 · Japan 01")["filter"].as_str(), Some("^Japan 01$"));
        assert_eq!(rules.last(), Some(&"MATCH,默认地区"));
    }

    #[test]
    fn group_regex_matches_like_desktop() {
        let defs = subscription::default_groups();
        let names = ["🇺🇸 美国 01", "US-LA", "Russia 01", "台湾 02", "TW 03", "Twitter 节点", "日本"];
        for d in &defs {
            let re = fancy(&subscription::group_regex(d).unwrap());
            for n in names {
                assert_eq!(re(n), subscription::group_matches(d, n), "group {} node {}", d.name, n);
            }
        }
    }

    /// 测试用：把 mihomo 的 (?<!..)/(?!..) 正则换成等价判断（Rust regex 不支持环视）
    fn fancy(re: &str) -> impl Fn(&str) -> bool {
        let body = re.trim_start_matches("(?i)(").trim_end_matches(')').to_string();
        let parts: Vec<String> = body.split('|').map(String::from).collect();
        move |name: &str| {
            parts.iter().any(|p| {
                if let Some(k) = p.strip_prefix("(?<![A-Za-z])").and_then(|x| x.strip_suffix("(?![A-Za-z])")) {
                    subscription::keyword_hits(&k.replace('\\', ""), name)
                } else if let Some(n) = p.strip_prefix('^').and_then(|x| x.strip_suffix('$')) {
                    name.eq_ignore_ascii_case(&n.replace('\\', ""))
                } else {
                    name.to_lowercase().contains(&p.replace('\\', "").to_lowercase())
                }
            })
        }
    }
}

#[cfg(test)]
mod live {
    //! 用真实订阅内容生成手机配置，写到 CFY_DUMP_MOBILE，给真实 mihomo 验证。需要 CFY_SUB_URL、CFY_SUB_TEXT
    use super::*;
    use crate::settings::{NodeGroup, Subscription};

    #[test]
    #[ignore]
    fn dump_live() {
        let url = std::env::var("CFY_SUB_URL").unwrap();
        let text = std::fs::read_to_string(std::env::var("CFY_SUB_TEXT").unwrap()).unwrap();
        let mut st = Settings::default();
        st.subscriptions = vec![Subscription { id: "s0".into(), url, ..Default::default() }];
        let mut g = subscription::default_groups();
        g.push(NodeGroup { id: "hk".into(), name: "香港".into(), keywords: vec!["香港".into(), "HK".into()], nodes: vec![] });
        g.push(NodeGroup { id: "my".into(), name: "精选".into(), keywords: vec![], nodes: vec!["SG-Premium".into()] });
        st.groups = Some(g);
        st.site_choices.insert("discord".into(), "node:🇯🇵 日本 东京".into());
        st.default_target = "region:香港".into();
        let texts = vec![text];
        let parsed = subscription::parse_many(&texts, &st.node_groups());
        let sites = crate::sites::catalog(&st);
        let yaml = generate(&MobileInput { settings: &st, parsed: &parsed, sites: &sites, texts: &texts }).unwrap();
        std::fs::write(std::env::var("CFY_DUMP_MOBILE").unwrap(), yaml).unwrap();
    }
}
