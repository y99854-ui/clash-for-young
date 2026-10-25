//! 网站目录：内置常用网站（sites.json）+ 用户自己添加的网站。

use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: String,
    pub cat: String,
    pub name: String,
    /// 分流检查时访问的网址
    pub url: String,
    pub domains: Vec<String>,
    #[serde(default)]
    pub custom: bool,
    /// 内置网站的域名被用户改过
    #[serde(default)]
    pub edited: bool,
    /// 用户没选过时的默认走向（科研版：Scholar、下载类网站默认走 "stable"）
    #[serde(default, rename = "default")]
    pub default_choice: String,
    /// 学校订阅的数据库 / 出版社：开启“我在校园网”时直连，用学校的订阅
    #[serde(default)]
    pub campus: bool,
}

pub fn builtin() -> &'static Vec<Site> {
    static S: OnceLock<Vec<Site>> = OnceLock::new();
    static R: OnceLock<Vec<Site>> = OnceLock::new();
    if crate::edition::is_research() {
        R.get_or_init(|| serde_json::from_str(include_str!("sites-research.json")).expect("sites-research.json"))
    } else {
        S.get_or_init(|| serde_json::from_str(include_str!("sites.json")).expect("sites.json"))
    }
}

/// 网站当前的走向：用户选过的，没选过就用默认
pub fn choice_of<'a>(st: &'a Settings, site: &'a Site) -> &'a str {
    st.site_choices.get(&site.id).map(|s| s.as_str()).unwrap_or(&site.default_choice)
}

pub const CUSTOM_CAT: &str = "我添加的";

pub fn catalog(st: &Settings) -> Vec<Site> {
    let mut out: Vec<Site> = builtin()
        .iter()
        .map(|s| {
            let mut s = s.clone();
            if let Some(d) = st.site_domains.get(&s.id) {
                if !d.is_empty() {
                    s.domains = d.clone();
                    s.edited = true;
                }
            }
            s
        })
        .collect();
    for c in &st.custom_sites {
        out.push(Site {
            id: c.id.clone(),
            cat: CUSTOM_CAT.into(),
            name: c.name.clone(),
            url: c.url.clone(),
            domains: c.domains.clone(),
            custom: true,
            edited: false,
            default_choice: String::new(),
            campus: false,
        });
    }
    out
}

/// 把用户输入的网址或域名整理成域名：去掉协议、路径、端口、开头的 www.
pub fn normalize_domain(input: &str) -> Option<String> {
    let t = input.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    let with_scheme = if t.contains("://") { t.clone() } else { format!("http://{t}") };
    let host = url::Url::parse(&with_scheme).ok()?.host_str()?.to_string();
    let host = host.trim_start_matches("*.").trim_start_matches('.').to_string();
    if !host.contains('.') && host != "localhost" {
        return None;
    }
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_loads_and_ids_unique() {
        let b = builtin();
        assert!(b.len() > 20);
        let mut ids: Vec<_> = b.iter().map(|s| &s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), b.len());
    }

    #[test]
    fn research_catalog() {
        let b: Vec<Site> = serde_json::from_str(include_str!("sites-research.json")).unwrap();
        let mut ids: Vec<_> = b.iter().map(|s| &s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), b.len(), "id 重复");
        // Scholar 必须排在 Google 前面：规则按顺序匹配，scholar.googleusercontent.com 不能被 Google 先吃掉
        let pos = |id: &str| b.iter().position(|s| s.id == id).unwrap();
        assert!(pos("scholar") < pos("google"));
        assert!(pos("gdocs") < pos("google"));
        assert!(b.iter().any(|s| s.campus) && b.iter().any(|s| s.default_choice == "stable"));
        for s in &b {
            assert!(!s.domains.is_empty() && !s.url.is_empty(), "{}", s.id);
            for d in &s.domains {
                assert!(!d.contains('/') && !d.contains(' ') && d.contains('.'), "{} {}", s.id, d);
            }
        }
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_domain("https://www.Example.com/a?b").unwrap(), "example.com");
        assert_eq!(normalize_domain("sub.foo.org:8443").unwrap(), "sub.foo.org");
        assert_eq!(normalize_domain("*.bar.net").unwrap(), "bar.net");
        assert!(normalize_domain("hello").is_none());
    }
}
