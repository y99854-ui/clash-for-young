//! 分流检查：经本软件的代理访问一次网址，从内核的活动连接里读出命中的规则和节点。
//! 逻辑沿用「分流检查」工具。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// 访问内核需要的东西
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Api {
    pub controller: String,
    pub secret: String,
    pub mixed_port: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    pub name: String,
    pub kind: String,
    pub now: String,
    pub all: Vec<String>,
    /// 只有 Selector 类型能手动切换；URLTest/Fallback 是自动选的
    pub selectable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitIp {
    pub ip: String,
    pub location: String,
    pub org: String,
    /// 查询出口 IP 这个请求本身走的节点
    pub via_node: String,
}

/// 一次“检查分流”的完整结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub url: String,
    pub host: String,
    pub http_status: Option<u16>,
    /// 本工具经代理请求该网址、收到响应头所用的毫秒数
    pub request_ms: u64,
    pub rule: String,
    pub rule_payload: String,
    /// 节点链：第一个是最终出口节点，后面依次是外层策略组
    pub chains: Vec<String>,
    pub node: String,
    pub node_type: String,
    /// mihomo 实际连接的远端地址（通常是节点服务器）
    pub remote_destination: String,
    /// 目标网站被解析到的 IP（走代理时可能为空，由节点远程解析）
    pub destination_ip: String,
    /// mihomo 延迟测试：经该节点访问该网址的往返时间
    pub node_delay_ms: Option<u64>,
    pub node_delay_error: Option<String>,
    pub exit_ip: Option<ExitIp>,
    pub exit_ip_error: Option<String>,
    pub groups: Vec<GroupInfo>,
    pub warnings: Vec<String>,
}

// ───────────────────────── 控制 API ─────────────────────────

/// 访问控制 API 的客户端：显式禁用代理，否则请求可能绕回 Clash 自己。
pub fn api_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

/// 走代理端口的客户端：模拟“普通应用经 Clash 上网”。
fn proxied_client(cfg: &Api) -> Result<reqwest::Client, String> {
    let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{}", cfg.mixed_port))
        .map_err(|e| e.to_string())?;
    reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(10))
        // 只看第一跳：分流按域名决定，跟随跳转只会多花时间、多出别的域名的连接
        .redirect(reqwest::redirect::Policy::none())
        // 让连接在请求结束后多留一会儿，方便去 /connections 里找到它
        .pool_idle_timeout(Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())
}

pub fn api_url(cfg: &Api, path: &str) -> String {
    format!("http://{}{}", cfg.controller, path)
}

pub fn with_auth(req: reqwest::RequestBuilder, cfg: &Api) -> reqwest::RequestBuilder {
    if cfg.secret.is_empty() {
        req
    } else {
        req.bearer_auth(&cfg.secret)
    }
}

pub async fn api_get(cfg: &Api, path: &str) -> Result<Value, String> {
    let client = api_client()?;
    let resp = with_auth(client.get(api_url(cfg, path)), cfg)
        .send()
        .await
        .map_err(|e| tr!("连不上控制 API {}：{e}", "Can't reach the control API {}: {e}", cfg.controller))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(tr!("控制 API 拒绝访问：密钥（secret）不对", "The control API refused access: wrong secret"));
    }
    if !status.is_success() {
        let msg = body.get("message").and_then(|m| m.as_str()).unwrap_or("");
        return Err(tr!("控制 API 返回 {status} {msg}", "The control API returned {status} {msg}"));
    }
    Ok(body)
}


pub fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// 当前活动连接里所有指向该主机的连接。
pub async fn host_connections(cfg: &Api, host: &str) -> Result<Vec<Value>, String> {
    let data = api_get(cfg, "/connections").await?;
    let conns = data
        .get("connections")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(conns
        .into_iter()
        .filter(|c| {
            let m = &c["metadata"];
            str_of(m, "host").eq_ignore_ascii_case(host) || str_of(m, "destinationIP") == host
        })
        .collect())
}

/// 在当前活动连接里找到目标主机最新的一条，跳过 exclude 里的连接 id（发请求前就已存在的连接）。
async fn find_connection(
    cfg: &Api,
    host: &str,
    exclude: &HashSet<String>,
) -> Result<Option<Value>, String> {
    Ok(host_connections(cfg, host)
        .await?
        .into_iter()
        .filter(|c| !exclude.contains(&str_of(c, "id")))
        // start 是 RFC3339 时间字符串，按字典序比较就是按时间先后
        .max_by(|a, b| str_of(a, "start").cmp(&str_of(b, "start"))))
}

fn chains_of(conn: &Value) -> Vec<String> {
    conn.get("chains")
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// 经代理请求一个 IP 回显服务，并查出这个请求自己走了哪个节点。
/// 返回出口 IP 信息和这个请求的原始节点链（由调用方统一顺序）
async fn query_exit_ip(cfg: &Api) -> Result<(ExitIp, Vec<String>), String> {
    let client = proxied_client(cfg)?;
    let host = "ipinfo.io";
    let resp = client
        .get("https://ipinfo.io/json")
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| tr!("查询出口 IP 失败：{e}", "Exit IP lookup failed: {e}"))?;
    // 先在连接还活着的时候查链路，再读响应体
    let via_chain = find_connection(cfg, host, &HashSet::new())
        .await
        .ok()
        .flatten()
        .map(|c| chains_of(&c))
        .unwrap_or_default();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| tr!("出口 IP 响应无法解析：{e}", "Couldn't read the exit IP response: {e}"))?;
    let location = [str_of(&body, "country"), str_of(&body, "region"), str_of(&body, "city")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    Ok((
        ExitIp {
            ip: str_of(&body, "ip"),
            location,
            org: str_of(&body, "org"),
            via_node: String::new(),
        },
        via_chain,
    ))
}

/// mihomo 的 chains 是“出口节点在前”；sing-box 等内核顺序可能相反。
/// 统一成节点在前：如果第一个是策略组（有 all 列表）而最后一个不是，就反转。
fn normalize_chains(mut chains: Vec<String>, pmap: &Value) -> Vec<String> {
    let is_group = |n: &String| pmap.get(n).and_then(|p| p.get("all")).is_some();
    if chains.len() > 1 && is_group(&chains[0]) && !is_group(&chains[chains.len() - 1]) {
        chains.reverse();
    }
    chains
}

/// 内核自带的延迟测试：让指定节点去访问 url，返回毫秒数。
async fn node_delay(cfg: &Api, node: &str, url: &str) -> Result<u64, String> {
    let path = format!(
        "/proxies/{}/delay?timeout=5000&url={}",
        urlencoding::encode(node),
        urlencoding::encode(url)
    );
    let v = api_get(cfg, &path).await?;
    v.get("delay")
        .and_then(|d| d.as_u64())
        .ok_or_else(|| tr!("延迟测试没有返回结果", "The latency test returned no result"))
}

fn normalize_url(input: &str) -> Result<url::Url, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err(tr!("请输入网址", "Enter a URL"));
    }
    let with_scheme = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let u = url::Url::parse(&with_scheme).map_err(|e| tr!("网址格式不对：{e}", "Invalid URL: {e}"))?;
    if u.host_str().is_none() {
        return Err(tr!("网址里没有域名", "The URL has no domain"));
    }
    Ok(u)
}


pub async fn inspect(cfg: Api, url: String, full: bool) -> Result<CheckResult, String> {
    let target = normalize_url(&url)?;
    let host = target.host_str().unwrap_or("").to_string();
    let target_str = target.to_string();
    let mut warnings = Vec::new();

    // 1. 先记下这个主机已经存在的连接（比如浏览器开着的），避免把它们误认成我们的
    let existing: HashSet<String> = host_connections(&cfg, &host)
        .await?
        .iter()
        .map(|c| str_of(c, "id"))
        .collect();

    // 2. 经代理真实访问一次（HEAD 请求），同时轮询 /connections。
    //    有些网站（如 Reddit、Netflix、Google Scholar）会拒绝非浏览器请求并立刻断开连接，
    //    等请求结束再去查就找不到了；所以在连接一建立、还没被断开时就把它抓下来。
    let client = proxied_client(&cfg)?;
    let done = AtomicBool::new(false);
    let request = async {
        let t0 = Instant::now();
        // HEAD：只要响应头、不下载页面，流量最小，也不像在抓取网页
        let r = client.head(target.clone()).send().await;
        let ms = t0.elapsed().as_millis() as u64;
        done.store(true, Ordering::SeqCst);
        (r, ms)
    };
    let poller = async {
        let mut polls_after_done = 0;
        loop {
            if let Ok(Some(c)) = find_connection(&cfg, &host, &existing).await {
                return Some(c);
            }
            if done.load(Ordering::SeqCst) {
                polls_after_done += 1;
                if polls_after_done > 3 {
                    return None;
                }
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    };
    let ((resp, request_ms), mut conn) = tokio::join!(request, poller);

    let http_status = match &resp {
        Ok(r) => Some(r.status().as_u16()),
        Err(e) => {
            let reason = if e.is_timeout() {
                tr!("超时", "timed out")
            } else if e.is_connect() {
                tr!("连接失败", "connection failed")
            } else {
                e.to_string()
            };
            warnings.push(tr!(
                "网站没有正常响应（{reason}）。分流信息仍然有效：这只说明经该节点访问这个网站有问题，或网站拒绝了非浏览器的请求。",
                "The site didn't respond normally ({reason}). The routing info is still valid: it only means this site had trouble through this node, or the site refuses non-browser requests."
            ));
            None
        }
    };
    if let Ok(r) = &resp {
        let code = r.status().as_u16();
        if code == 403 || code == 429 {
            warnings.push(tr!(
                "网站返回 HTTP {code}：通常是网站拒绝了自动化请求或该节点 IP，分流信息仍然有效；浏览器里实际能否打开以浏览器为准。",
                "The site returned HTTP {code}: it usually rejects automated requests or this node's IP. The routing info is still valid; whether it opens in your browser is what counts."
            ));
        }
    }
    drop(resp);

    // 3. 实在没抓到新连接，就退而用已存在的连接（同一主机通常命中同一条规则）
    if conn.is_none() && !existing.is_empty() {
        conn = find_connection(&cfg, &host, &HashSet::new()).await?;
        if conn.is_some() {
            warnings.push(tr!("没抓到本次请求的连接，显示的是该网站已有连接（例如浏览器）的分流信息。", "Couldn't catch this request's connection; showing routing for an existing connection to this site (e.g. from the browser)."));
        }
    }

    let Some(conn) = conn else {
        return Err(tr!(
            "请求已发出，但在代理内核的活动连接里找不到 {host}。\n\
             请确认已连接，并且没有其他代理软件占用端口 {}。",
            "The request was sent, but {host} isn't among the proxy core's active connections.\n\
             Make sure you're connected and no other proxy app is using port {}.",
            cfg.mixed_port
        ));
    };

    let meta = &conn["metadata"];

    // 3. 读取所有代理/策略组，把链路上每个组的信息取出来
    let proxies = api_get(&cfg, "/proxies").await?;
    let pmap = &proxies["proxies"];

    // mihomo 的 chains 是“出口节点在前”；不同内核/版本顺序可能相反。
    // 统一成节点在前：如果第一个是策略组（有 all 列表）而最后一个不是，就反转。
    let chains = normalize_chains(chains_of(&conn), pmap);
    let node = chains.first().cloned().unwrap_or_default();
    let node_type = str_of(&pmap[&node], "type");
    let groups = chains
        .iter()
        .skip(1)
        .filter_map(|g| {
            let p = pmap.get(g)?;
            let kind = str_of(p, "type");
            Some(GroupInfo {
                name: g.clone(),
                selectable: kind == "Selector",
                kind,
                now: str_of(p, "now"),
                all: p
                    .get("all")
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
            })
        })
        .collect();

    // 4. 节点延迟和出口 IP 互不依赖，同时进行（快速模式跳过出口 IP）
    let exit_fut = async {
        if full {
            Some(query_exit_ip(&cfg).await)
        } else {
            None
        }
    };
    let (delay, exit) = tokio::join!(node_delay(&cfg, &node, &target_str), exit_fut);

    let (exit_ip, exit_ip_error) = match exit {
        None => (None, None),
        Some(Ok((mut e, via_chain))) => {
            e.via_node = normalize_chains(via_chain, pmap).into_iter().next().unwrap_or_default();
            if !e.via_node.is_empty() && e.via_node != node {
                warnings.push(tr!(
                    "出口 IP 查询走的是「{}」，不是「{node}」，所以下面的出口 IP 不代表本网址的出口。",
                    "The exit IP lookup went through \"{}\", not \"{node}\", so the exit IP below isn't this URL's exit.",
                    e.via_node
                ));
            }
            (Some(e), None)
        }
        Some(Err(e)) => (None, Some(e)),
    };
    let (node_delay_ms, node_delay_error) = match delay {
        Ok(d) => (Some(d), None),
        Err(e) => (None, Some(e)),
    };

    Ok(CheckResult {
        url: target_str,
        host,
        http_status,
        request_ms,
        rule: str_of(&conn, "rule"),
        rule_payload: str_of(&conn, "rulePayload"),
        chains,
        node,
        node_type,
        remote_destination: str_of(meta, "remoteDestination"),
        destination_ip: str_of(meta, "destinationIP"),
        node_delay_ms,
        node_delay_error,
        exit_ip,
        exit_ip_error,
        groups,
        warnings,
    })
}
