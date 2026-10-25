// Clash for Young / Clash for Research 界面逻辑（同一套代码，S.edition 区分）。所有操作都通过 invoke() 交给 Rust 后端。
const { invoke } = window.__TAURI__.core;

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

let S = null;          // 后端返回的完整状态
let status = null;     // 首页状态（当前节点、延迟）
let view = "home";
let rulesDraft = [];   // 正在编辑的自定义规则
let delays = {};       // 节点延迟

// ───────── 中英文 ─────────
let LANG = "zh";       // 实际使用的语言，来自后端 S.lang
let APP = "Clash for Young";  // 软件名，来自后端 S.appName（科研版是 Clash for Research）
const isResearch = () => S?.edition === "research";
const isEn = () => LANG === "en";
function L(zh, en) { return isEn() ? en : zh; }
// 软件内部的名字（配置里的组名、内置网站分类和中文网站名）：英文界面显示英文
const NAME_EN = {
  "默认地区": "Default region", "自动选择": "Auto (fastest)", "其他": "Other", "直连": "Direct",
  "美国": "United States", "台湾": "Taiwan",
  "视频": "Video", "社交": "Social", "常用": "Everyday", "科研": "Research", "国内": "China", "我添加的": "Added by me",
  "稳定线路": "Stable line",
  "文献检索": "Literature search", "出版社": "Publishers", "代码与数据": "Code & data", "写作工具": "Writing tools",
  "会议交流": "Meetings & chat", "国内学术": "China academic",
  "万方数据": "Wanfang Data", "维普": "CQVIP", "百度学术": "Baidu Scholar", "超星": "Chaoxing",
  "中国知网": "CNKI", "哔哩哔哩": "Bilibili", "百度": "Baidu", "淘宝": "Taobao", "知乎": "Zhihu", "微信": "WeChat", "小红书": "Xiaohongshu (RED)",
};
const dn = (name) => (isEn() && NAME_EN[name]) || name;
// 页面里写死的 “Clash for Young” 换成当前软件名（科研版）
function fixAppName() {
  document.title = APP;
  if (APP === "Clash for Young") return;
  const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let n;
  while ((n = w.nextNode())) if (n.nodeValue.includes("Clash for Young")) n.nodeValue = n.nodeValue.replaceAll("Clash for Young", APP);
  for (const el of document.querySelectorAll("[title]")) el.title = el.title.replaceAll("Clash for Young", APP);
}
// 把页面上带 data-en 的固定文字切换到当前语言
function applyLang() {
  document.documentElement.lang = isEn() ? "en" : "zh-CN";
  for (const [attr, key, zhKey] of [["innerHTML", "en", "zh"], ["placeholder", "enPlaceholder", "zhPlaceholder"], ["title", "enTitle", "zhTitle"]]) {
    const sel = key === "en" ? "[data-en]" : key === "enPlaceholder" ? "[data-en-placeholder]" : "[data-en-title]";
    document.querySelectorAll(sel).forEach((el) => {
      if (el.dataset[zhKey] == null) el.dataset[zhKey] = el[attr];
      el[attr] = isEn() ? el.dataset[key] : el.dataset[zhKey];
    });
  }
  fixAppName();
  $("welcomeLang").innerHTML = isEn()
    ? `<button type="button" class="linklike" data-lang="zh">切换到中文</button>`
    : `<button type="button" class="linklike" data-lang="en">Switch to English</button>`;
}

// ───────── 小工具 ─────────
let toastTimer;
function toast(msg, bad = false) {
  const t = $("toast");
  t.textContent = msg;
  t.className = "toast" + (bad ? " bad" : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add("hidden"), bad ? 6000 : 2500);
}

async function call(cmd, args = {}, { quiet = false } = {}) {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    if (!quiet) toast(String(e), true);
    throw e;
  }
}

function fmtBytes(n) {
  if (!n) return "0";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (i >= 3 ? n.toFixed(1) : Math.round(n)) + " " + u[i];
}
function fmtDate(ms) {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
function fmtAgo(ms) {
  const m = Math.round((Date.now() - ms) / 60000);
  if (m < 1) return L("刚刚", "just now");
  if (m < 60) return L(`${m} 分钟前`, `${m} min ago`);
  if (m < 60 * 24) return L(`${Math.round(m / 60)} 小时前`, `${Math.round(m / 60)} h ago`);
  return L(`${Math.round(m / 1440)} 天前`, `${Math.round(m / 1440)} days ago`);
}
function delayClass(d) { return d == null ? "" : d < 300 ? "delay-good" : d < 800 ? "delay-mid" : "delay-slow"; }

// ───────── 走向下拉框 ─────────
// mode: "site"（网站页，含“自动”）/ "default"（首页）/ "rule"（自定义规则）
function targetOptions(selected, mode) {
  const opts = [];
  const add = (v, label) => opts.push(`<option value="${esc(v)}"${v === selected ? " selected" : ""}>${esc(label)}</option>`);
  if (mode === "site") add("", L("自动", "Auto"));
  if (mode === "rule") add("default", L("默认地区", "Default region"));
  if (mode !== "default" && isResearch()) add("stable", L("稳定线路（选好节点后不再换）", "Stable line (sticks to one node)"));
  if (mode === "default") add("fastest", L("自动（最快的节点）", "Auto (fastest node)"));
  else add("fastest", L("最快的节点", "Fastest node"));
  add("DIRECT", mode === "default" ? L("全部直连（不走代理）", "All direct (no proxy)") : L("直连", "Direct"));
  if (mode === "rule") add("REJECT", L("拦截", "Block"));
  const regions = S?.regions || [];
  if (regions.length) {
    opts.push(`<optgroup label="${L("分组（组内自动选最快）", "Groups (fastest node in the group)")}">`);
    for (const r of regions) add("region:" + r.name, `${dn(r.name)} (${r.nodes.length})`);
    opts.push(`</optgroup>`);
  }
  for (const r of regions) {
    opts.push(`<optgroup label="${L("具体节点", "Nodes")} · ${esc(dn(r.name))}">`);
    for (const n of r.nodes) add("node:" + n, n);
    opts.push(`</optgroup>`);
  }
  // 已失效的选择也要显示出来，避免下拉框悄悄跳到别的选项
  if (selected && !opts.some((o) => o.includes(" selected"))) {
    opts.push(`<option value="${esc(selected)}" selected>${L("（已失效）", "(unavailable) ")}${esc(selected.replace(/^(region|node):/, ""))}</option>`);
  }
  return opts.join("");
}

// ───────── 视图切换 ─────────
function show(v) {
  view = v;
  for (const id of ["welcome", "home", "sites", "settings"]) $("view-" + id).classList.toggle("hidden", id !== v);
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t.dataset.view === v));
  $("tabs").classList.toggle("hidden", v === "welcome");
}
document.querySelectorAll(".tab").forEach((t) => t.addEventListener("click", () => { show(t.dataset.view); render(); }));

// ───────── 渲染 ─────────
function versionText(v) { return v === "dev" ? L("开发版", "dev build") : v || ""; }
function render() {
  if (!S) return;
  if ((S.lang || "zh") !== LANG || !$("welcomeLang").innerHTML || (S.appName && S.appName !== APP)) {
    LANG = S.lang || "zh";
    APP = S.appName || APP;
    document.body.classList.toggle("edition-research", isResearch());
    applyLang();
  }
  renderLang();
  if (!S.subscriptions.length) { show("welcome"); return; }
  if (view === "welcome") show("home");
  $("appVersion").textContent = versionText(S.version);
  renderUpdate();
  renderHome();
  renderSites();
  renderSettings();
}

function renderHome() {
  const on = S.connected;
  $("powerBtn").classList.toggle("on", on);
  const cat = on ? "assets/cat-awake.png" : "assets/cat-sleep.png";
  if (!$("catImg").src.endsWith(cat)) { $("catImg").src = cat; $("brandCat").src = cat; }
  $("connText").textContent = on ? L("已连接", "Connected") : L("未连接", "Not connected");
  $("tunTag").classList.toggle("hidden", !(on && S.tun.enabled && S.tun.privileged));
  $("globalTag").classList.toggle("hidden", !(on && S.globalMode));
  document.querySelectorAll("#modeSeg button").forEach((b) => b.classList.toggle("active", (b.dataset.global === "1") === S.globalMode));
  $("modeHint").textContent = S.globalMode
    ? L("全局：所有网站（包括国内网站）都走首页的默认地区，下面的设置暂时不生效。", "Global: every site, including China sites, uses the default region on the Home page. The settings below are paused.")
    : L("规则：国内网站直连，其他网站按下面的设置走。", "Rule: China sites go direct; other sites follow the settings below.");
  $("defaultHint").textContent = S.globalMode
    ? L("全局模式下所有网站都走这里（局域网除外）。分组可以在「设置 → 节点分组」里修改。", "In Global mode every site uses this (except your local network). Edit groups in Settings → Node groups.")
    : L("没有单独设置的网站都走这里。国内网站始终直连。分组可以在「设置 → 节点分组」里修改。", "Sites without their own setting use this. China sites always go direct. Edit groups in Settings → Node groups.");
  let now = "";
  if (on && status) {
    if (status.node === "DIRECT") now = L("全部直连", "All direct");
    else if (status.node) {
      const d = status.delay != null ? ` · <span class="${delayClass(status.delay)}">${status.delay} ms</span>` : "";
      now = `${esc(dn(status.region))} · ${esc(status.node)}${d}`;
    }
  } else if (!on) now = L("小猫在睡觉，点它开始上网", "The cat is asleep. Click it to get online.");
  $("nowText").innerHTML = now;
  const showStable = isResearch() && on && status && status.stable;
  $("stableText").classList.toggle("hidden", !showStable);
  if (showStable) $("stableText").textContent = L(`稳定线路（Scholar、下载）· ${status.stable}`, `Stable line (Scholar, downloads) · ${status.stable}`);

  const err = status?.error || S.error;
  $("homeError").classList.toggle("hidden", !err);
  $("homeError").textContent = err || "";

  $("defaultSelect").innerHTML = targetOptions(S.defaultTarget, "default");

  const subs = S.subscriptions;
  $("subInfo").textContent = subsCount(subs.length, S.nodeCount);
  $("homeSubs").innerHTML = subs.map((sub) => {
    const u = sub.usage;
    const right = [];
    if (u && u.total) right.push(usedText(u));
    if (u && u.expire) right.push(expireText(u));
    const bar = u && u.total
      ? `<div class="usage"><div style="width:${Math.min(100, ((u.upload + u.download) / u.total) * 100)}%"></div></div>` : "";
    return `<div class="home-sub"><div class="line"><span><b>${esc(sub.name)}</b> · ${nodesText(sub.nodeCount)}</span><span>${esc(right.join(" · "))}</span></div>${bar}</div>`;
  }).join("");
}

const nodesText = (n) => L(`${n} 个节点`, n === 1 ? "1 node" : `${n} nodes`);
const subsCount = (subs, nodes) => L(`${subs} 个订阅 · 共 ${nodes} 个节点`, `${subs} subscription${subs === 1 ? "" : "s"} · ${nodes} nodes`);
const usedText = (u) => L(`已用 ${fmtBytes(u.upload + u.download)} / ${fmtBytes(u.total)}`, `Used ${fmtBytes(u.upload + u.download)} / ${fmtBytes(u.total)}`);
const expireText = (u) => L(`${fmtDate(u.expire * 1000)} 到期`, `expires ${fmtDate(u.expire * 1000)}`);
function subMeta(sub) {
  const ago = fmtAgo(sub.updatedAt);
  const parts = [nodesText(sub.nodeCount), sub.url ? L(`更新于 ${ago}`, `updated ${ago}`) : L(`导入于 ${ago}`, `imported ${ago}`)];
  const u = sub.usage;
  if (u && u.total) parts.push(usedText(u));
  if (u && u.expire) parts.push(expireText(u));
  return parts.join(" · ");
}

function renderSubs() {
  const subs = S.subscriptions;
  $("subsSummary").textContent = subsCount(subs.length, S.nodeCount);
  $("subsList").innerHTML = subs.map((sub) => `<div class="sub-row">
      <div><div class="sub-name">${esc(sub.name)}${sub.url ? "" : `<span class="tag-file">${L("文件", "file")}</span>`}</div>
        <div class="sub-meta" title="${esc(sub.url || L("从文件导入", "Imported from a file"))}">${esc(subMeta(sub))}</div></div>
      <div class="sub-ops">
        ${sub.url ? `<button class="ghost" data-sop="refresh" data-sid="${esc(sub.id)}">${L("更新", "Update")}</button>` : ""}
        <button class="ghost" data-sop="rename" data-sid="${esc(sub.id)}">${L("改名", "Rename")}</button>
        <button class="ghost danger" data-sop="del" data-sid="${esc(sub.id)}">${L("删除", "Delete")}</button>
      </div></div>`).join("");
}

const CAT_ORDER_YOUNG = ["AI", "视频", "社交", "常用", "科研", "国内", "我添加的"];
const CAT_ORDER_RESEARCH = ["文献检索", "出版社", "代码与数据", "写作工具", "AI", "会议交流", "常用", "国内学术", "我添加的"];
function renderSites() {
  const fb = S.fallbacks || [];
  $("fallbackWarn").classList.toggle("hidden", !fb.length);
  $("fallbackWarn").textContent = fb.length
    ? L(`订阅里已经没有这些选择对应的节点或地区，暂时改走默认地区：${fb.join("、")}`, `These choices point to nodes or groups no longer in your subscriptions, so they use the default region for now: ${fb.map(dn).join(", ")}`)
    : "";

  const cats = {};
  for (const s of S.sites) (cats[s.cat] ||= []).push(s);
  const CAT_ORDER = isResearch() ? CAT_ORDER_RESEARCH : CAT_ORDER_YOUNG;
  const order = Object.keys(cats).sort((a, b) => (CAT_ORDER.indexOf(a) + 99) % 99 - (CAT_ORDER.indexOf(b) + 99) % 99);
  // 校园网
  $("campusCard").classList.toggle("hidden", !isResearch());
  $("campusToggle").checked = !!S.campusMode;
  $("siteGroups").innerHTML = order
    .map((cat) => `<div class="cat-title">${esc(dn(cat))}</div><div class="card site-card">${cats[cat]
      .map((s) => `<div class="site-row" data-id="${esc(s.id)}">
          <div><div class="site-name" data-edit="${esc(s.id)}" title="${L("点击编辑包含的域名", "Click to edit the domains")}">${esc(dn(s.name))}${s.edited ? `<span class="edited-tag">${L("已改", "edited")}</span>` : ""}</div>
            <div class="site-sub">${esc(s.domains.join("  "))}</div></div>
          ${campusDirect(s)
            ? `<select disabled title="${L("校园网直连中", "Direct on campus")}"><option>${L("直连（校园网）", "Direct (campus)")}</option></select>`
            : `<select data-choice="${esc(s.id)}" class="${s.choice ? "set" : ""}">${targetOptions(s.choice, "site")}</select>`}
          <div class="site-actions"><button class="ghost small-btn" data-check="${esc(s.id)}">${L("检查", "Check")}</button></div>
          <div class="site-check hidden" id="chk-${esc(s.id)}"></div>
        </div>`)
      .join("")}</div>`)
    .join("");

  // 自定义规则：只有在没编辑时才用后端数据刷新
  if (!$("rulesBox").dataset.dirty) rulesDraft = (S.customRules || []).map((r) => ({ ...r }));
  renderRules();

  $("siteList").innerHTML = S.sites.map((s) => `<option value="${esc(s.url)}">${esc(dn(s.name))}</option>`).join("");
}

const RULE_KINDS = () => [
  ["suffix", L("域名（含子域名）", "Domain (with subdomains)")],
  ["domain", L("完整域名", "Exact domain")],
  ["keyword", L("域名关键词", "Domain keyword")],
  ["ipcidr", L("IP / IP 段", "IP / IP range")],
];
function renderRules() {
  $("rulesList").innerHTML = rulesDraft.length
    ? rulesDraft.map((r, i) => `<div class="rule-row" data-i="${i}">
        <select data-k="kind">${RULE_KINDS().map(([v, l]) => `<option value="${v}"${v === r.kind ? " selected" : ""}>${l}</option>`).join("")}</select>
        <input data-k="value" value="${esc(r.value)}" placeholder="${r.kind === "ipcidr" ? "1.2.3.0/24" : r.kind === "keyword" ? "google" : "example.com"}" spellcheck="false" />
        <select data-k="target">${targetOptions(r.target || "default", "rule")}</select>
        <div class="ord"><button class="ghost" data-op="up" title="${L("上移", "Move up")}">↑</button><button class="ghost" data-op="down" title="${L("下移", "Move down")}">↓</button><button class="ghost danger" data-op="del" title="${L("删除", "Delete")}">✕</button></div>
      </div>`).join("")
    : `<p class="muted small">${L("还没有自定义规则。", "No custom rules yet.")}</p>`;
}

const campusDirect = (s) => isResearch() && S.campusMode && s.campus && !S.globalMode;

// 终端代理：复制给已经打开的终端用的命令
function termCommands() {
  const u = `http://127.0.0.1:${S.mixedPort}`;
  return {
    unix: [L("复制 Mac / Linux 命令", "Copy Mac / Linux command"), `export http_proxy=${u} https_proxy=${u} HTTP_PROXY=${u} HTTPS_PROXY=${u}`],
    ps: [L("复制 PowerShell 命令", "Copy PowerShell command"), `$env:HTTP_PROXY="${u}"; $env:HTTPS_PROXY="${u}"`],
    cmd: [L("复制 CMD 命令", "Copy CMD command"), `set HTTP_PROXY=${u}&& set HTTPS_PROXY=${u}`],
  };
}
function renderTerm() {
  $("termBox").classList.toggle("hidden", !isResearch());
  if (!isResearch()) return;
  $("termToggle").checked = !!S.terminalProxy;
  $("termSummary").textContent = S.terminalProxy ? L("已开启", "On") : L("关闭", "Off");
  const keys = S.tun.platform === "windows" ? ["ps", "cmd"] : ["unix"];
  const c = termCommands();
  $("termCopy").innerHTML = keys.map((k) => `<button type="button" class="ghost small-btn" data-copy="${k}">${esc(c[k][0])}</button>`).join("");
}

function renderSettings() {
  renderTerm();
  if (document.activeElement !== $("mixedPort")) $("mixedPort").value = S.mixedPort;
  if (document.activeElement !== $("ctlPort")) $("ctlPort").value = S.controllerPort;
  $("dataDir").textContent = L("数据目录：", "Data folder: ") + S.dataDir;
  renderSubs();
  renderTun();
  renderGroups();
  renderNodes();
}

function renderNodes() {
  $("nodesList").innerHTML = (S.regions || [])
    .map((r) => `<div class="region-block"><div class="region-name">${esc(dn(r.name))} <span class="muted small">${r.nodes.length}</span></div>
      <div class="node-grid">${r.nodes.map((n) => {
        const d = delays[n];
        const txt = d == null ? (Object.keys(delays).length ? L("超时", "timeout") : "") : d + " ms";
        return `<div class="node-item"><span class="n" title="${esc(n)}">${esc(n)}</span><span class="${delayClass(d)}">${txt}</span></div>`;
      }).join("")}</div></div>`)
    .join("");
}

function renderGroups() {
  const gs = S.groups || [];
  $("groupsList").innerHTML = gs.length
    ? gs.map((g, i) => `<div class="group-row">
        <div><div class="group-title">${esc(dn(g.name))} <span class="muted small">${nodesText(g.matched.length)}</span>${g.matched.length ? "" : `<span class="empty-tag">${L("没有匹配的节点，暂不可选", "No matching nodes, not selectable yet")}</span>`}</div>
          <div class="group-kw" title="${esc([...g.keywords, ...g.nodes].join("\n"))}">${esc(g.keywords.length ? L("关键词：", "Keywords: ") + g.keywords.join("  ") : "")}${g.keywords.length && g.nodes.length ? " · " : ""}${g.nodes.length ? L(`手动选了 ${g.nodes.length} 个节点`, `${g.nodes.length} picked by hand`) : ""}</div></div>
        <div class="group-ops">
          <button class="ghost" data-gop="up" data-gid="${esc(g.id)}" ${i === 0 ? "disabled" : ""} title="${L("上移", "Move up")}">↑</button>
          <button class="ghost" data-gop="down" data-gid="${esc(g.id)}" ${i === gs.length - 1 ? "disabled" : ""} title="${L("下移", "Move down")}">↓</button>
          <button class="ghost" data-gop="edit" data-gid="${esc(g.id)}">${L("编辑", "Edit")}</button>
          <button class="ghost danger" data-gop="del" data-gid="${esc(g.id)}">${L("删除", "Delete")}</button>
        </div></div>`).join("")
    : `<p class="muted small">${L("还没有分组，全部节点都在「其他」里。", "No groups yet; all nodes are in “Other”.")}</p>`;
  $("groupsSummary").textContent = L(`${gs.length} 个分组`, `${gs.length} group${gs.length === 1 ? "" : "s"}`);
  const other = (S.regions || []).find((r) => !r.id);
  $("otherInfo").textContent = other
    ? L(`还有 ${other.nodes.length} 个节点不在任何分组，归入「其他」。`, `${other.nodes.length} node(s) aren't in any group and go under “Other”.`)
    : L("所有节点都已分组。", "All nodes are grouped.");
}

function renderTun() {
  const t = S.tun;
  const active = t.enabled && t.privileged;
  $("tunToggle").checked = t.enabled;
  $("tunSummary").textContent = active ? L("已开启", "On") : t.enabled ? L("未生效", "Not active") : L("关闭", "Off");
  let hint = "", fix = "";
  if (t.platform === "mac") {
    hint = L("第一次开启会弹出系统密码框（输入开机密码），给代理内核授权。软件更新后需要再授权一次。", "The first time, macOS asks for your login password to authorize the proxy core. You'll need to authorize again after each app update.");
    if (t.enabled && !t.privileged) { hint = L("⚠️ 增强模式还没生效：代理内核需要重新授权（更新软件后会这样）。", "⚠️ Enhanced mode isn't active yet: the proxy core needs to be authorized again (this happens after an app update)."); fix = L("授权", "Authorize"); }
  } else if (t.platform === "windows") {
    hint = L(`需要以管理员身份运行 ${APP}。开启后软件会以管理员身份重新启动（会弹出确认窗口）。`, `${APP} needs to run as administrator. Turning this on restarts the app as administrator (Windows will ask you to confirm).`);
    if (t.enabled && !t.privileged) { hint = L("⚠️ 增强模式还没生效：需要以管理员身份运行。", "⚠️ Enhanced mode isn't active yet: the app needs to run as administrator."); fix = L("以管理员身份重新启动", "Restart as administrator"); }
  } else {
    hint = L("这个系统暂不支持增强模式。", "Enhanced mode isn't supported on this system.");
    $("tunToggle").disabled = true;
  }
  $("tunHint").textContent = hint;
  $("tunHint").classList.toggle("tun-warn", !!fix);
  $("tunFixBtn").textContent = fix;
  $("tunFixBtn").classList.toggle("hidden", !fix);
}

// ───────── 数据刷新 ─────────
async function refreshState() {
  S = await call("get_state", {}, { quiet: true });
  render();
}
let tick = 0;
async function pollStatus() {
  tick++;
  try {
    const withDelay = tick % 6 === 1;
    const st = await invoke("status", { withDelay });
    if (!withDelay && status && st.node === status.node) st.delay = status.delay;
    status = st;
    if (S && (S.connected !== st.connected || S.running !== st.running)) await refreshState();
    else if (S) renderHome();
  } catch (_) {}
}

// ───────── 事件 ─────────
$("welcomeForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const url = $("welcomeUrl").value.trim();
  if (!url) return;
  $("welcomeBtn").disabled = true;
  $("welcomeMsg").textContent = L("正在下载订阅…", "Downloading subscription…");
  try {
    S = await call("add_subscription", { url }, { quiet: true });
    $("welcomeMsg").textContent = L(`找到 ${S.nodeCount} 个节点，正在连接…`, `Found ${S.nodeCount} nodes, connecting…`);
    try { S = await call("connect", {}, { quiet: true }); } catch (err) { toast(String(err), true); }
    show("home");
    render();
    tick = 0; pollStatus();
  } catch (err) {
    $("welcomeMsg").textContent = String(err);
  } finally {
    $("welcomeBtn").disabled = false;
  }
});

// 欢迎页：从文件导入，导入后自动连接
$("welcomeFileBtn").addEventListener("click", () => {
  afterImport = async () => {
    try { S = await call("connect", {}, { quiet: true }); } catch (err) { toast(String(err), true); }
    show("home");
    tick = 0; pollStatus();
  };
  $("subFileInput").click();
});

$("powerBtn").addEventListener("click", async () => {
  const btn = $("powerBtn");
  btn.classList.add("busy"); btn.disabled = true;
  $("connText").textContent = S.connected ? L("正在断开…", "Disconnecting…") : L("正在连接…", "Connecting…");
  try {
    S = await call(S.connected ? "disconnect" : "connect");
    status = null; tick = 0;
    await pollStatus();
  } catch (_) {
  } finally {
    btn.classList.remove("busy"); btn.disabled = false;
    render();
  }
});

$("defaultSelect").addEventListener("change", async (e) => {
  try {
    S = await call("set_default_target", { target: e.target.value });
    tick = 0; await pollStatus();
    toast(L("已切换默认地区", "Default region changed"));
  } catch (_) { render(); }
});

async function refreshSub(id) {
  toast(L("正在更新订阅…", "Updating subscriptions…"));
  try {
    S = await call("refresh_subscription", { id: id || null });
    render();
    toast(L(`订阅已更新：共 ${S.nodeCount} 个节点`, `Subscriptions updated: ${S.nodeCount} nodes`));
  } catch (_) {
    await refreshState(); // 部分失败时其他订阅可能已更新
  }
}
$("refreshBtn").addEventListener("click", () => refreshSub());
$("refreshAllBtn").addEventListener("click", () => refreshSub());
$("subForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const url = $("subUrl").value.trim();
  if (!url) return;
  toast(L("正在下载订阅…", "Downloading subscription…"));
  try {
    S = await call("add_subscription", { url });
    $("subUrl").value = "";
    render();
    toast(L(`已添加：现在共 ${S.nodeCount} 个节点`, `Added: ${S.nodeCount} nodes in total`));
  } catch (_) {}
});

// 从文件导入：读出文本交给后端（可一次选多个文件）
let afterImport = null;
$("subFileBtn").addEventListener("click", () => { afterImport = null; $("subFileInput").click(); });
$("subFileInput").addEventListener("change", async (e) => {
  const files = [...e.target.files];
  e.target.value = "";
  let ok = 0;
  for (const f of files) {
    try {
      const content = await f.text();
      S = await call("import_subscription_file", { name: f.name, content });
      ok++;
    } catch (_) {}
  }
  if (ok) {
    toast(L(`已导入 ${ok} 个文件：现在共 ${S.nodeCount} 个节点`, `Imported ${ok} file(s): ${S.nodeCount} nodes in total`));
    if (afterImport) await afterImport();
    render();
  }
});

$("subsList").addEventListener("click", async (e) => {
  const op = e.target.dataset.sop, id = e.target.dataset.sid;
  if (!op) return;
  const sub = S.subscriptions.find((x) => x.id === id);
  if (op === "refresh") return refreshSub(id);
  if (op === "rename") {
    const name = await askText(L(`给「${sub.name}」改个名字`, `Rename “${sub.name}”`), sub.name);
    if (!name) return;
    try { S = await call("rename_subscription", { id, name }); render(); } catch (_) {}
  }
  if (op === "del") {
    const last = S.subscriptions.length === 1;
    const msg = L(`删除订阅「${sub.name}」？它的 ${sub.nodeCount} 个节点会被移除。${last ? "这是最后一个订阅，删除后会断开连接。" : ""}`,
      `Delete subscription “${sub.name}”? Its ${sub.nodeCount} nodes will be removed.${last ? " This is your last subscription, so you'll be disconnected." : ""}`);
    if (!(await askConfirm(msg))) return;
    try { S = await call("remove_subscription", { id }); render(); } catch (_) {}
  }
});

// 网站页：选择走向 / 检查 / 编辑
$("siteGroups").addEventListener("change", async (e) => {
  const id = e.target.dataset.choice;
  if (!id) return;
  try {
    S = await call("set_site_choice", { id, target: e.target.value });
    renderSites();
    toast(L("已生效", "Applied"));
  } catch (_) { renderSites(); }
});
$("siteGroups").addEventListener("click", async (e) => {
  const cid = e.target.dataset.check;
  const eid = e.target.closest("[data-edit]")?.dataset.edit;
  if (cid) {
    const site = S.sites.find((s) => s.id === cid);
    const box = $("chk-" + cid);
    box.classList.remove("hidden");
    box.innerHTML = `<span class="muted">${L("正在检查…", "Checking…")}</span>`;
    try {
      const r = await call("check_url", { url: site.url }, { quiet: true });
      const d = r.nodeDelayMs != null ? ` · <span class="${delayClass(r.nodeDelayMs)}">${r.nodeDelayMs} ms</span>` : "";
      const via = r.node === "DIRECT" ? L("直连", "Direct") : `${esc(r.node)}`;
      box.innerHTML = `${L("实际走", "Goes via")} <b>${via}</b>${d} <span class="muted">(${L("规则", "rule")}: ${esc(r.rule)} ${esc(r.rulePayload)})</span>`;
    } catch (err) {
      box.innerHTML = `<span class="delay-slow">${esc(err)}</span>`;
    }
  } else if (eid) {
    openSiteDialog(S.sites.find((s) => s.id === eid));
  }
});

$("addSiteForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const url = $("addSiteUrl").value.trim();
  if (!url) return;
  try {
    S = await call("save_site", { site: { id: null, name: $("addSiteName").value.trim(), url, domains: [url] } });
    $("addSiteUrl").value = ""; $("addSiteName").value = "";
    renderSites();
    toast(L("已添加到「我添加的」，可以给它选择走向", "Added under “Added by me”. Choose a route for it."));
  } catch (_) {}
});

// 编辑网站对话框
let editing = null;
function openSiteDialog(site) {
  editing = site;
  $("sdTitle").textContent = site.custom ? L("编辑网站", "Edit site") : L(`编辑「${site.name}」的域名`, `Edit domains for “${dn(site.name)}”`);
  $("sdName").value = site.name;
  $("sdUrl").value = site.url;
  $("sdName").disabled = $("sdUrl").disabled = !site.custom;
  $("sdDomains").value = site.domains.join("\n");
  $("sdDelete").textContent = site.custom ? L("删除网站", "Delete site") : L("恢复默认域名", "Restore default domains");
  $("sdDelete").classList.toggle("hidden", !site.custom && !site.edited);
  $("siteDialog").showModal();
}
$("sdCancel").addEventListener("click", () => $("siteDialog").close());
$("siteDialogForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const domains = $("sdDomains").value.split(/\n/).map((x) => x.trim()).filter(Boolean);
  try {
    S = await call("save_site", { site: { id: editing.id, name: $("sdName").value, url: $("sdUrl").value, domains } });
    $("siteDialog").close();
    renderSites();
    toast(L("已保存", "Saved"));
  } catch (_) {}
});
$("sdDelete").addEventListener("click", async () => {
  try {
    S = await call("delete_site", { id: editing.id });
    $("siteDialog").close();
    renderSites();
  } catch (_) {}
});

// 自定义规则
function markRulesDirty() { $("rulesBox").dataset.dirty = "1"; }
$("addRuleBtn").addEventListener("click", () => {
  rulesDraft.push({ kind: "suffix", value: "", target: "default" });
  markRulesDirty(); renderRules();
  $("rulesList").querySelector(".rule-row:last-child input")?.focus();
});
$("rulesList").addEventListener("input", (e) => {
  const row = e.target.closest(".rule-row");
  if (!row || !e.target.dataset.k) return;
  rulesDraft[+row.dataset.i][e.target.dataset.k] = e.target.value;
  markRulesDirty();
  if (e.target.dataset.k === "kind") renderRules();
});
$("rulesList").addEventListener("click", (e) => {
  const op = e.target.dataset.op;
  const row = e.target.closest(".rule-row");
  if (!op || !row) return;
  const i = +row.dataset.i;
  if (op === "del") rulesDraft.splice(i, 1);
  if (op === "up" && i > 0) [rulesDraft[i - 1], rulesDraft[i]] = [rulesDraft[i], rulesDraft[i - 1]];
  if (op === "down" && i < rulesDraft.length - 1) [rulesDraft[i + 1], rulesDraft[i]] = [rulesDraft[i], rulesDraft[i + 1]];
  markRulesDirty(); renderRules();
});
$("saveRulesBtn").addEventListener("click", async () => {
  try {
    S = await call("set_custom_rules", { rules: rulesDraft });
    delete $("rulesBox").dataset.dirty;
    renderSites();
    toast(L("规则已保存并生效", "Rules saved and applied"));
  } catch (_) {}
});

// 设置页：分流检查
$("checkForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const url = $("checkUrl").value.trim();
  if (!url) return;
  $("checkBtn").disabled = true;
  $("checkResult").innerHTML = `<p class="muted">${L("正在经代理访问并读取分流信息…", "Opening the URL through the proxy and reading the routing info…")}</p>`;
  try {
    const r = await call("check_url", { url }, { quiet: true });
    const chain = r.chains.map((c) => `<span class="chip">${esc(dn(c))}</span>`).join(`<span class="arrow">←</span>`);
    const exit = r.exitIp ? `${esc(r.exitIp.ip)} <span class="muted">${esc(r.exitIp.location)} ${esc(r.exitIp.org)}</span>` : esc(r.exitIpError || "");
    $("checkResult").innerHTML = `<div class="check-res">
      ${r.warnings.map((w) => `<div class="warn">${esc(w)}</div>`).join("")}
      <div class="kv"><span class="k">${L("节点", "Node")}</span><b>${esc(r.node === "DIRECT" ? L("直连", "Direct") : r.node)}</b></div>
      <div class="kv"><span class="k">${L("链路", "Chain")}</span><div class="chain">${chain}</div></div>
      <div class="kv"><span class="k">${L("命中规则", "Matched rule")}</span><span class="mono">${esc(r.rule)} ${esc(r.rulePayload)}</span></div>
      <div class="kv"><span class="k">${L("节点延迟", "Node latency")}</span><span class="${delayClass(r.nodeDelayMs)}">${r.nodeDelayMs != null ? r.nodeDelayMs + " ms" : esc(r.nodeDelayError || "-")}</span></div>
      <div class="kv"><span class="k">${L("本次请求", "This request")}</span><span>${r.requestMs} ms${r.httpStatus ? " · HTTP " + r.httpStatus : ""}</span></div>
      <div class="kv"><span class="k">${L("出口 IP", "Exit IP")}</span><span class="mono">${exit}</span></div>
    </div>`;
  } catch (err) {
    $("checkResult").innerHTML = `<div class="error">${esc(err)}</div>`;
  } finally {
    $("checkBtn").disabled = false;
  }
});

$("delayBtn").addEventListener("click", async () => {
  $("delayBtn").disabled = true;
  $("delayMsg").textContent = L("测试中，最多 30 秒…", "Testing, up to 30 seconds…");
  try {
    delays = (await call("node_delays")) || {};
    $("delayMsg").textContent = L(`${Object.keys(delays).length} 个节点可用`, `${Object.keys(delays).length} nodes available`);
    renderNodes();
  } catch (_) {
    $("delayMsg").textContent = "";
  } finally {
    $("delayBtn").disabled = false;
  }
});

$("portsBtn").addEventListener("click", async () => {
  try {
    S = await call("set_ports", { mixedPort: +$("mixedPort").value, controllerPort: +$("ctlPort").value });
    render(); toast(L("端口已保存", "Ports saved"));
  } catch (_) {}
});
$("logBtn").addEventListener("click", async () => {
  const box = $("logBox");
  box.textContent = (await call("core_log")) || L("（暂无日志）", "(no log yet)");
  box.classList.remove("hidden");
});
$("cfgBtn").addEventListener("click", async () => {
  const box = $("logBox");
  box.textContent = (await call("current_config")) || L("（还没有生成配置）", "(no config generated yet)");
  box.classList.remove("hidden");
});

// 增强模式
async function setTun(enabled) {
  $("tunToggle").disabled = true;
  try {
    if (enabled) toast(S.tun.platform === "mac" ? L("请在弹出的窗口里输入开机密码…", "Enter your login password in the prompt…") : L("正在开启增强模式…", "Turning on Enhanced mode…"));
    S = await call("set_tun", { enabled }, { quiet: true });
    toast(enabled ? L("增强模式已开启", "Enhanced mode is on") : L("增强模式已关闭", "Enhanced mode is off"));
  } catch (err) {
    if (String(err) === "NEED_ADMIN") {
      await refreshState();
      if (await askConfirm(L(`增强模式需要以管理员身份运行。现在以管理员身份重新启动 ${APP} 吗？（会弹出系统确认窗口）`, `Enhanced mode needs administrator rights. Restart ${APP} as administrator now? (Windows will ask you to confirm.)`))) {
        try { await call("relaunch_as_admin"); } catch (_) {}
      }
    } else {
      toast(String(err), true);
    }
  } finally {
    $("tunToggle").disabled = false;
    render();
  }
}
$("tunToggle").addEventListener("change", (e) => setTun(e.target.checked));

// 导出手机配置
$("exportMobileBtn").addEventListener("click", async () => {
  try {
    const path = await call("export_mobile_config");
    $("exportMobileInfo").textContent = L("已保存到「下载」文件夹：", "Saved to your Downloads folder: ") + path.split(/[\\/]/).pop();
    toast(L("已导出，文件在「下载」文件夹", "Exported to your Downloads folder"));
  } catch (_) {}
});
$("tunFixBtn").addEventListener("click", async () => {
  if (S.tun.platform === "windows") {
    try { await call("relaunch_as_admin"); } catch (_) {}
  } else {
    setTun(true);
  }
});

// 确认框（不用 window.confirm，部分系统的网页视图不支持）
function askConfirm(text) {
  return new Promise((resolve) => {
    const d = $("confirmDialog");
    $("cfText").textContent = text;
    const done = (v) => { d.close(); $("cfYes").onclick = $("cfNo").onclick = null; resolve(v); };
    $("cfYes").onclick = () => done(true);
    $("cfNo").onclick = () => done(false);
    d.oncancel = () => done(false);
    d.showModal();
  });
}

// 输入框（改名用）
function askText(title, value) {
  return new Promise((resolve) => {
    const d = $("textDialog");
    $("tdTitle").textContent = title;
    $("tdInput").value = value || "";
    const done = (v) => { d.close(); $("tdOk").onclick = $("tdNo").onclick = null; resolve(v); };
    $("tdOk").onclick = () => done($("tdInput").value.trim());
    $("tdNo").onclick = () => done(null);
    d.oncancel = () => done(null);
    d.showModal();
    $("tdInput").select();
  });
}

// 节点分组
let editingGroup = null;
let gdManual = new Set();
let gdMatched = new Set();
function allNodeNames() { return (S.regions || []).flatMap((r) => r.nodes); }
function renderGroupNodes() {
  const f = $("gdFilter").value.trim().toLowerCase();
  const kw = new Set(gdMatched);
  $("gdNodes").innerHTML = allNodeNames()
    .filter((n) => !f || n.toLowerCase().includes(f))
    .map((n) => {
      const byKw = kw.has(n) && !gdManual.has(n);
      return `<label class="gd-node${byKw ? " kw" : ""}"><input type="checkbox" data-node="${esc(n)}" ${gdManual.has(n) || byKw ? "checked" : ""} ${byKw ? "disabled" : ""}/>
        <span>${esc(n)}</span>${byKw ? `<span class="why">${L("关键词", "keyword")}</span>` : ""}</label>`;
    }).join("") || `<span class="muted small">${L("没有节点", "No nodes")}</span>`;
  $("gdCount").textContent = L(`共 ${gdMatched.size} 个节点`, `${gdMatched.size} nodes`);
}
let previewTimer;
async function previewGroup() {
  clearTimeout(previewTimer);
  previewTimer = setTimeout(async () => {
    try {
      gdMatched = new Set(await invoke("preview_group", { keywords: $("gdKeywords").value, nodes: [...gdManual] }));
    } catch (_) {}
    renderGroupNodes();
  }, 150);
}
function openGroupDialog(g) {
  editingGroup = g || null;
  $("gdTitle").textContent = g ? L(`编辑分组「${g.name}」`, `Edit group “${g.name}”`) : L("新建分组", "New group");
  $("gdName").value = g ? g.name : "";
  $("gdKeywords").value = g ? g.keywords.join(", ") : "";
  $("gdFilter").value = "";
  gdManual = new Set(g ? g.nodes : []);
  gdMatched = new Set(g ? g.matched : []);
  renderGroupNodes();
  $("groupDialog").showModal();
  previewGroup();
}
$("addGroupBtn").addEventListener("click", () => openGroupDialog(null));
$("gdCancel").addEventListener("click", () => $("groupDialog").close());
$("gdKeywords").addEventListener("input", previewGroup);
$("gdFilter").addEventListener("input", renderGroupNodes);
$("gdNodes").addEventListener("change", (e) => {
  const n = e.target.dataset.node;
  if (!n) return;
  e.target.checked ? gdManual.add(n) : gdManual.delete(n);
  previewGroup();
});
$("groupForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    S = await call("save_group", { group: { id: editingGroup?.id || null, name: $("gdName").value, keywords: $("gdKeywords").value, nodes: [...gdManual] } });
    $("groupDialog").close();
    render();
    toast(L("分组已保存并生效", "Group saved and applied"));
  } catch (_) {}
});
$("groupsList").addEventListener("click", async (e) => {
  const op = e.target.dataset.gop, id = e.target.dataset.gid;
  if (!op) return;
  const g = S.groups.find((x) => x.id === id);
  try {
    if (op === "edit") return openGroupDialog(g);
    if (op === "del") {
      if (!(await askConfirm(L(`删除分组「${g.name}」？选了这个分组的网站会改走默认地区。`, `Delete group “${dn(g.name)}”? Sites that use it will switch to the default region.`)))) return;
      S = await call("delete_group", { id });
    }
    if (op === "up" || op === "down") S = await call("move_group", { id, delta: op === "up" ? -1 : 1 });
    render();
  } catch (_) {}
});
$("resetGroupsBtn").addEventListener("click", async () => {
  if (!(await askConfirm(L("恢复成默认的「美国」「台湾」两个分组？你新建的分组会被删除。", "Restore the default “United States” and “Taiwan” groups? Groups you created will be deleted.")))) return;
  try { S = await call("reset_groups"); render(); toast(L("已恢复默认分组", "Default groups restored")); } catch (_) {}
});

// ───────── 规则 / 全局 ─────────
$("modeSeg").addEventListener("click", async (e) => {
  const b = e.target.closest("button");
  if (!b) return;
  const global = b.dataset.global === "1";
  if (global === S.globalMode) return;
  try {
    S = await call("set_global_mode", { global });
    render();
    toast(global ? L("已切换到全局模式", "Switched to Global mode") : L("已切换到规则模式", "Switched to Rule mode"));
  } catch (_) {}
});

// ───────── 更新 ─────────
let updateTimer = null;
function renderUpdate() {
  const u = S.update;
  const dev = S.version === "dev";
  const cur = versionText(S.version);
  $("updateDot").classList.toggle("hidden", !u);
  $("updateSummary").textContent = u ? L(`有新版本 ${u.version}`, `New version ${u.version}`) : dev ? cur : L(`当前 ${cur}`, `Current ${cur}`);
  let text;
  if (!u) text = dev ? L("本地开发版不检查更新。", "Local development builds don't check for updates.")
    : L(`现在是 ${cur}。软件会定期在后台检查新版本，有更新时这里会出现红点。`, `You're on ${cur}. The app checks for new versions in the background; a red dot appears here when one is available.`);
  else if (u.installing) {
    text = u.total && u.downloaded >= u.total ? L("正在安装，装好后会自动重新打开…", "Installing. The app will reopen when it's done…")
      : u.total ? L(`正在下载 ${fmtBytes(u.downloaded)} / ${fmtBytes(u.total)}…`, `Downloading ${fmtBytes(u.downloaded)} / ${fmtBytes(u.total)}…`)
      : L(`正在下载 ${fmtBytes(u.downloaded)}…`, `Downloading ${fmtBytes(u.downloaded)}…`);
  } else if (u.error) text = u.error;
  else text = L(`新版本 ${u.version} 可以更新（现在是 ${cur}）。更新时会短暂断开，软件重新打开后自动连上。`, `Version ${u.version} is available (you're on ${cur}). You'll be disconnected briefly during the update; the app reconnects when it reopens.`);
  $("updateText").textContent = text;
  $("updateText").classList.toggle("muted", !(u && u.error));
  $("updateBtn").classList.toggle("hidden", !u);
  $("updateBtn").disabled = !!(u && u.installing);
  $("updateBtn").textContent = u && u.installing ? L("更新中…", "Updating…") : u && u.error ? L("重试", "Retry") : L("一键更新", "Update now");
  $("checkUpdateBtn").disabled = dev || !!(u && u.installing);
  $("updateProgress").classList.toggle("hidden", !(u && u.installing));
  $("updateProgressBar").style.width = u && u.total ? `${Math.min(100, (u.downloaded / u.total) * 100)}%` : "5%";
  $("updateNotesBox").classList.toggle("hidden", !(u && u.notes));
  $("updateNotes").textContent = (u && u.notes) || "";
  // 下载中每半秒刷新进度
  if (u && u.installing && !updateTimer) updateTimer = setInterval(refreshState, 500);
  if (!(u && u.installing) && updateTimer) { clearInterval(updateTimer); updateTimer = null; }
}
$("updateBtn").addEventListener("click", async () => {
  const u = S.update;
  if (!u) return;
  S.update = { ...u, installing: true, downloaded: 0, total: 0, error: null };
  renderUpdate();
  try {
    await call("install_update", {}, { quiet: true });
  } catch (_) {}
  await refreshState(); // 失败时显示错误；成功时软件已重启，不会走到这里
});
$("checkUpdateBtn").addEventListener("click", async () => {
  toast(L("正在检查更新…", "Checking for updates…"));
  try {
    S = await call("check_update");
    render();
    toast(S.update ? L(`发现新版本 ${S.update.version}`, `Found version ${S.update.version}`) : L("已经是最新版本", "You're up to date"));
  } catch (_) {}
});

// ───────── 左上角回首页 ─────────
$("brandBtn").addEventListener("click", () => {
  if (view === "welcome") return;
  show("home");
  render();
  window.scrollTo(0, 0);
});

// ───────── 分享：复制下载链接（不含订阅链接） ─────────

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (_) {
    // 部分系统的网页视图不允许直接写剪贴板，退回老办法
    const t = document.createElement("textarea");
    t.value = text;
    t.style.position = "fixed"; t.style.opacity = "0";
    document.body.appendChild(t);
    t.select();
    const ok = document.execCommand("copy");
    t.remove();
    return ok;
  }
}
$("shareBtn").addEventListener("click", async () => {
  const text = S?.downloadUrl || "https://github.com/y99854-ui/clash-for-young/releases/latest";
  if (await copyText(text)) toast(L("已复制下载链接，粘贴发给朋友吧", "Download link copied. Paste it to a friend."));
  else await askText(L("复制下面的链接发给朋友", "Copy this link and send it to a friend"), text);
});

// ───────── 科研版：校园网、终端代理 ─────────
$("campusToggle").addEventListener("change", async (e) => {
  const on = e.target.checked;
  try {
    S = await call("set_campus_mode", { on });
    render();
    toast(on ? L("校园网模式已开启：出版社和数据库直连", "Campus mode on: publishers and databases go direct") : L("校园网模式已关闭", "Campus mode off"));
  } catch (_) { render(); }
});
$("termToggle").addEventListener("change", async (e) => {
  const on = e.target.checked;
  try {
    S = await call("set_terminal_proxy", { on });
    render();
    toast(on
      ? (S.connected ? L("已开启：新打开的终端会走代理", "On: new terminals will use the proxy") : L("已开启：连接后新打开的终端会走代理", "On: new terminals will use the proxy once you connect"))
      : L("已关闭终端代理", "Terminal proxy off"));
  } catch (_) { render(); }
});
$("termCopy").addEventListener("click", async (e) => {
  const k = e.target.dataset.copy;
  if (!k) return;
  const cmd = termCommands()[k][1];
  if (await copyText(cmd)) toast(L("已复制，粘贴到终端里回车即可", "Copied. Paste it into the terminal and press Enter."));
  else await askText(L("复制下面的命令", "Copy this command"), cmd);
});

// ───────── 语言 ─────────
function renderLang() {
  $("langSelect").value = S.language || "";
  $("langSummary").textContent = !S.language ? L("跟随系统", "Follow system") : S.language === "en" ? "English" : "中文";
}
async function setLanguage(language) {
  try {
    S = await call("set_language", { language });
    render();
  } catch (_) {}
}
$("langSelect").addEventListener("change", (e) => setLanguage(e.target.value));
$("welcomeLang").addEventListener("click", (e) => { const l = e.target.dataset.lang; if (l) setLanguage(l); });

// ───────── 启动 ─────────
(async () => {
  await refreshState();
  if (S && S.subscriptions.length) show("home");
  render();
  pollStatus();
  setInterval(pollStatus, 5000);
  setInterval(refreshState, 60000);
})();
