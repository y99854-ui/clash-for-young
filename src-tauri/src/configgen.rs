//! 根据 订阅节点 + 内置规则模板 + 用户选择 生成 mihomo 配置。

use crate::settings::{CustomRule, Settings};
use crate::sites::Site;
use crate::subscription::Parsed;
use serde_yaml::{Mapping, Value};

pub const GROUP_DEFAULT: &str = "默认地区";
pub const GROUP_FASTEST: &str = "自动选择";
const TEST_URL: &str = "https://www.gstatic.com/generate_204";

/// 内网地址：始终直连
const PRIVATE_CIDRS: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "224.0.0.0/4",
];

pub struct GenInput<'a> {
    pub settings: &'a Settings,
    pub parsed: &'a Parsed,
    pub sites: &'a [Site],
    /// 地理数据文件是否就绪（GeoSite.dat 和 geoip.metadb）
    pub geo_ready: bool,
    /// 开启 TUN（已开启且内核有权限时才为真）
    pub tun: bool,
}

pub struct GenOutput {
    pub yaml: String,
    /// 走向已不存在（比如订阅更新后节点没了），改走默认地区的条目
    pub fallbacks: Vec<String>,
}

/// 把界面里的走向翻译成配置里的名字。None 表示“跟随默认”，不用写规则。
pub fn resolve_target(target: &str, parsed: &Parsed) -> Result<Option<String>, ()> {
    let t = target.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let name = match t {
        "fastest" => GROUP_FASTEST.to_string(),
        "default" => GROUP_DEFAULT.to_string(),
        "DIRECT" | "REJECT" => t.to_string(),
        _ => {
            if let Some(r) = t.strip_prefix("region:") {
                if parsed.regions.iter().any(|x| x.name == r) {
                    r.to_string()
                } else {
                    return Err(());
                }
            } else if let Some(n) = t.strip_prefix("node:") {
                if parsed.node_names().iter().any(|x| x == n) {
                    n.to_string()
                } else {
                    return Err(());
                }
            } else {
                return Err(());
            }
        }
    };
    Ok(Some(name))
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

/// 规则值里不能有逗号、换行，否则会破坏规则格式
fn clean_value(v: &str) -> Option<String> {
    let v = v.trim().trim_start_matches("*.").trim_start_matches('.').to_string();
    if v.is_empty() || v.contains(',') || v.contains('\n') || v.contains(' ') {
        None
    } else {
        Some(v)
    }
}

pub fn rule_line(rule: &CustomRule, target_name: &str) -> Option<String> {
    let v = clean_value(&rule.value)?;
    Some(match rule.kind.as_str() {
        "domain" => format!("DOMAIN,{v},{target_name}"),
        "keyword" => format!("DOMAIN-KEYWORD,{v},{target_name}"),
        "ipcidr" => {
            let kind = if v.contains(':') { "IP-CIDR6" } else { "IP-CIDR" };
            let v = if v.contains('/') { v } else if v.contains(':') { format!("{v}/128") } else { format!("{v}/32") };
            format!("{kind},{v},{target_name},no-resolve")
        }
        _ => format!("DOMAIN-SUFFIX,{v},{target_name}"),
    })
}

pub fn generate(input: &GenInput) -> GenOutput {
    let st = input.settings;
    let parsed = input.parsed;
    let mut fallbacks = vec![];
    let all_nodes = parsed.node_names();
    let region_names: Vec<String> = parsed.regions.iter().map(|r| r.name.clone()).collect();

    // ── 策略组 ──
    let mut groups: Vec<Value> = vec![];

    // 默认地区：首页下拉框。选中的放第一个，内核启动时就选它。
    let default_name = match resolve_target(&st.default_target, parsed) {
        Ok(Some(n)) if n != GROUP_DEFAULT => n,
        Ok(_) => GROUP_FASTEST.to_string(),
        Err(()) => {
            fallbacks.push(format!("默认地区（{}）", display_target(&st.default_target)));
            GROUP_FASTEST.to_string()
        }
    };
    let mut default_members = vec![GROUP_FASTEST.to_string()];
    default_members.extend(region_names.iter().cloned());
    default_members.push("DIRECT".into());
    default_members.extend(all_nodes.iter().cloned());
    default_members.retain(|x| x != &default_name);
    default_members.insert(0, default_name);
    groups.push(map(vec![
        ("name", s(GROUP_DEFAULT)),
        ("type", s("select")),
        ("proxies", seq(default_members)),
    ]));

    let url_test = |name: &str, nodes: Vec<String>| {
        map(vec![
            ("name", s(name)),
            ("type", s("url-test")),
            ("proxies", seq(nodes)),
            ("url", s(TEST_URL)),
            ("interval", Value::from(300)),
            ("tolerance", Value::from(50)),
            ("lazy", Value::from(true)),
        ])
    };
    groups.push(url_test(GROUP_FASTEST, all_nodes.clone()));
    for r in &parsed.regions {
        groups.push(url_test(&r.name, r.nodes.clone()));
    }

    // ── 规则 ──
    let mut rules: Vec<String> = vec![];

    // 1. 自定义规则（用户排好的顺序）
    for r in &st.custom_rules {
        match resolve_target(&r.target, parsed) {
            Ok(Some(t)) => rules.extend(rule_line(r, &t)),
            Ok(None) => rules.extend(rule_line(r, GROUP_DEFAULT)),
            Err(()) => {
                fallbacks.push(format!("自定义规则 {}", r.value));
                rules.extend(rule_line(r, GROUP_DEFAULT));
            }
        }
    }

    // 2. 网站页的选择
    for site in input.sites {
        let Some(choice) = st.site_choices.get(&site.id) else { continue };
        let t = match resolve_target(choice, parsed) {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            Err(()) => {
                fallbacks.push(site.name.clone());
                GROUP_DEFAULT.to_string()
            }
        };
        for d in &site.domains {
            if let Some(d) = clean_value(d) {
                rules.push(format!("DOMAIN-SUFFIX,{d},{t}"));
            }
        }
    }

    // 3. 内置：内网和国内直连，其余走默认地区
    rules.push("DOMAIN-SUFFIX,local,DIRECT".into());
    rules.push("DOMAIN-SUFFIX,localhost,DIRECT".into());
    for c in PRIVATE_CIDRS {
        rules.push(format!("IP-CIDR,{c},DIRECT,no-resolve"));
    }
    rules.push("IP-CIDR6,::1/128,DIRECT,no-resolve".into());
    rules.push("IP-CIDR6,fe80::/10,DIRECT,no-resolve".into());
    if input.geo_ready {
        rules.push("GEOSITE,cn,DIRECT".into());
        // no-resolve：只对直接用 IP 的连接判断，不为了判断规则去本地解析外网域名
        // （本地解析会把访问的网站暴露给本地 DNS，也会拖慢每个新连接）
        rules.push("GEOIP,CN,DIRECT,no-resolve".into());
    }
    // 走代理的 QUIC（UDP 443）拦掉，浏览器会自动改用 TCP：很多节点转发 UDP 很慢
    rules.push("AND,((NETWORK,UDP),(DST-PORT,443)),REJECT".into());
    rules.push(format!("MATCH,{GROUP_DEFAULT}"));

    // ── 整体 ──
    // DNS：
    // - 国内网站用国内 DNS（直连），其他网站用国外 DNS 并且经过代理（respect-rules），
    //   不会把访问的国外网站泄漏给本地网络，也能拿到离节点近的 IP
    // - 增强模式用 fake-ip：程序拿到的是假 IP，真实解析交给节点完成
    let cn_dns = vec!["https://223.5.5.5/dns-query".to_string(), "https://1.12.12.12/dns-query".to_string()];
    let mut policy = Mapping::new();
    policy.insert(s("+.lan"), Value::from("system"));
    policy.insert(s("+.local"), Value::from("system"));
    if input.geo_ready {
        policy.insert(s("geosite:cn,private"), seq(cn_dns.clone()));
    }
    let dns = map(vec![
        ("enable", Value::from(true)),
        ("ipv6", Value::from(false)),
        ("enhanced-mode", s(if input.tun { "fake-ip" } else { "redir-host" })),
        ("fake-ip-range", s("198.18.0.1/16")),
        (
            "fake-ip-filter",
            seq(vec![
                "+.lan".into(),
                "+.local".into(),
                "+.msftconnecttest.com".into(),
                "+.msftncsi.com".into(),
                "time.*.com".into(),
                "+.ntp.org".into(),
                "+.stun.*.*".into(),
                "+.stun.*.*.*".into(),
                "localhost.ptlogin2.qq.com".into(),
            ]),
        ),
        ("respect-rules", Value::from(true)),
        ("default-nameserver", seq(vec!["223.5.5.5".into(), "119.29.29.29".into()])),
        ("nameserver", seq(vec!["https://1.1.1.1/dns-query".into(), "https://8.8.8.8/dns-query".into()])),
        ("nameserver-policy", Value::Mapping(policy)),
        // 解析节点服务器地址用（直连）
        ("proxy-server-nameserver", seq(cn_dns)),
    ]);
    let geox = map(vec![
        ("geoip", s("https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/geoip.dat")),
        ("geosite", s("https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/geosite.dat")),
        ("mmdb", s("https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/country-lite.mmdb")),
    ]);
    let profile = map(vec![("store-selected", Value::from(false))]);
    let tun = map(vec![
        ("enable", Value::from(input.tun)),
        ("stack", s("mixed")),
        ("auto-route", Value::from(true)),
        ("auto-detect-interface", Value::from(true)),
        ("strict-route", Value::from(false)),
        ("dns-hijack", seq(vec!["any:53".into()])),
    ]);

    let proxies: Vec<Value> = parsed.proxies.iter().cloned().map(Value::Mapping).collect();
    let root = map(vec![
        ("mixed-port", Value::from(st.mixed_port)),
        ("allow-lan", Value::from(false)),
        ("bind-address", s("127.0.0.1")),
        ("mode", s("rule")),
        ("log-level", s("warning")),
        ("ipv6", Value::from(false)),
        ("unified-delay", Value::from(true)),
        ("tcp-concurrent", Value::from(true)),
        // 不按程序分流，关掉进程查找：每个新连接都查进程会明显拖慢速度
        ("find-process-mode", s("off")),
        ("external-controller", Value::from(format!("127.0.0.1:{}", st.controller_port))),
        ("secret", Value::from(st.secret.clone())),
        ("geo-auto-update", Value::from(false)),
        ("geox-url", geox),
        ("profile", profile),
        ("dns", dns),
        ("tun", tun),
        ("proxies", Value::Sequence(proxies)),
        ("proxy-groups", Value::Sequence(groups)),
        ("rules", seq(rules)),
    ]);
    let yaml = format!(
        "# 由 Clash for Young 自动生成，每次修改设置都会覆盖。请不要手动编辑。\n{}",
        serde_yaml::to_string(&root).unwrap_or_default()
    );
    GenOutput { yaml, fallbacks }
}

pub fn display_target(t: &str) -> String {
    match t {
        "" => "自动".into(),
        "fastest" => "自动（最快）".into(),
        "default" => "默认地区".into(),
        "DIRECT" => "直连".into(),
        "REJECT" => "拦截".into(),
        _ => t.strip_prefix("region:").or_else(|| t.strip_prefix("node:")).unwrap_or(t).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::{default_groups, parse};

    fn sample() -> Parsed {
        let mut defs = default_groups();
        for (id, name) in [("hk", "香港"), ("jp", "日本")] {
            defs.push(crate::settings::NodeGroup { id: id.into(), name: name.into(), keywords: vec![name.into()], nodes: vec![] });
        }
        defs.retain(|g| g.id != "us" && g.id != "tw");
        parse(
            r#"
proxies:
  - {name: "香港 01", type: ss, server: a.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "日本 01", type: ss, server: b.com, port: 1, cipher: aes-128-gcm, password: x}
  - {name: "Mars", type: ss, server: c.com, port: 1, cipher: aes-128-gcm, password: x}
"#,
            &defs,
        )
        .unwrap()
    }

    #[test]
    fn rules_order_and_targets() {
        let parsed = sample();
        let mut st = Settings::default();
        st.site_choices.insert("discord".into(), "region:日本".into());
        st.site_choices.insert("google".into(), "node:香港 01".into());
        st.site_choices.insert("netflix".into(), "region:美国".into()); // 没有美国节点
        st.custom_rules.push(CustomRule { kind: "keyword".into(), value: "openai".into(), target: "DIRECT".into() });
        let sites = crate::sites::catalog(&st);
        let out = generate(&GenInput { settings: &st, parsed: &parsed, sites: &sites, geo_ready: true, tun: false });
        let v: Value = serde_yaml::from_str(&out.yaml).unwrap();
        let rules: Vec<String> =
            v["rules"].as_sequence().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        assert_eq!(rules[0], "DOMAIN-KEYWORD,openai,DIRECT");
        assert!(rules.contains(&"DOMAIN-SUFFIX,discordapp.net,日本".to_string()));
        assert!(rules.contains(&"DOMAIN-SUFFIX,gstatic.com,香港 01".to_string()));
        assert!(rules.contains(&"DOMAIN-SUFFIX,netflix.com,默认地区".to_string()));
        assert_eq!(rules.last().unwrap(), "MATCH,默认地区");
        assert!(out.fallbacks.contains(&"Netflix".to_string()));
        let groups = v["proxy-groups"].as_sequence().unwrap();
        let names: Vec<&str> = groups.iter().map(|g| g["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["默认地区", "自动选择", "香港", "日本", "其他"]);
        assert_eq!(groups[0]["proxies"][0].as_str().unwrap(), "自动选择");
    }

    #[test]
    fn default_target_first() {
        let parsed = sample();
        let mut st = Settings::default();
        st.default_target = "region:日本".into();
        let out = generate(&GenInput { settings: &st, parsed: &parsed, sites: &[], geo_ready: false, tun: true });
        let v: Value = serde_yaml::from_str(&out.yaml).unwrap();
        assert_eq!(v["proxy-groups"][0]["proxies"][0].as_str().unwrap(), "日本");
        assert!(!out.yaml.contains("GEOSITE"));
        assert_eq!(v["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(v["dns"]["enhanced-mode"].as_str(), Some("fake-ip"));
        if let Ok(p) = std::env::var("CFY_DUMP_TUN") {
            std::fs::write(p, &out.yaml).unwrap();
        }
    }

    #[test]
    fn dns_no_leak() {
        let parsed = sample();
        let st = Settings::default();
        let out = generate(&GenInput { settings: &st, parsed: &parsed, sites: &[], geo_ready: true, tun: false });
        let v: Value = serde_yaml::from_str(&out.yaml).unwrap();
        // 国外网站的 DNS 走代理，国内网站用国内 DNS
        assert_eq!(v["dns"]["respect-rules"].as_bool(), Some(true));
        assert!(v["dns"]["nameserver"][0].as_str().unwrap().contains("1.1.1.1"));
        assert!(v["dns"]["nameserver-policy"]["geosite:cn,private"].is_sequence());
        assert!(out.yaml.contains("GEOIP,CN,DIRECT,no-resolve"));
        assert!(!out.yaml.contains("doh.pub"));
        assert_eq!(v["find-process-mode"].as_str(), Some("off"));
    }

    #[test]
    fn ip_rules() {
        let r = CustomRule { kind: "ipcidr".into(), value: "1.2.3.4".into(), target: "DIRECT".into() };
        assert_eq!(rule_line(&r, "DIRECT").unwrap(), "IP-CIDR,1.2.3.4/32,DIRECT,no-resolve");
        let bad = CustomRule { kind: "suffix".into(), value: "a,b".into(), target: "DIRECT".into() };
        assert!(rule_line(&bad, "DIRECT").is_none());
    }
}
