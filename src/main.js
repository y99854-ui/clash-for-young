// Clash for Young 界面逻辑。所有操作都通过 invoke() 交给 Rust 后端。
const { invoke } = window.__TAURI__.core;

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

let S = null;          // 后端返回的完整状态
let status = null;     // 首页状态（当前节点、延迟）
let view = "home";
let rulesDraft = [];   // 正在编辑的自定义规则
let delays = {};       // 节点延迟

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
  if (m < 1) return "刚刚";
  if (m < 60) return `${m} 分钟前`;
  if (m < 60 * 24) return `${Math.round(m / 60)} 小时前`;
  return `${Math.round(m / 1440)} 天前`;
}
function delayClass(d) { return d == null ? "" : d < 300 ? "delay-good" : d < 800 ? "delay-mid" : "delay-slow"; }

// ───────── 走向下拉框 ─────────
// mode: "site"（网站页，含“自动”）/ "default"（首页）/ "rule"（自定义规则）
function targetOptions(selected, mode) {
  const opts = [];
  const add = (v, label) => opts.push(`<option value="${esc(v)}"${v === selected ? " selected" : ""}>${esc(label)}</option>`);
  if (mode === "site") add("", "自动");
  if (mode === "rule") add("default", "默认地区");
  if (mode === "default") add("fastest", "自动（最快的节点）");
  else add("fastest", "最快的节点");
  add("DIRECT", mode === "default" ? "全部直连（不走代理）" : "直连");
  if (mode === "rule") add("REJECT", "拦截");
  const regions = S?.regions || [];
  if (regions.length) {
    opts.push(`<optgroup label="分组（组内自动选最快）">`);
    for (const r of regions) add("region:" + r.name, `${r.name}（${r.nodes.length}）`);
    opts.push(`</optgroup>`);
  }
  for (const r of regions) {
    opts.push(`<optgroup label="具体节点 · ${esc(r.name)}">`);
    for (const n of r.nodes) add("node:" + n, n);
    opts.push(`</optgroup>`);
  }
  // 已失效的选择也要显示出来，避免下拉框悄悄跳到别的选项
  if (selected && !opts.some((o) => o.includes(" selected"))) {
    opts.push(`<option value="${esc(selected)}" selected>（已失效）${esc(selected.replace(/^(region|node):/, ""))}</option>`);
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
function render() {
  if (!S) return;
  if (!S.subscriptions.length) { show("welcome"); return; }
  if (view === "welcome") show("home");
  $("appVersion").textContent = S.version || "";
  renderHome();
  renderSites();
  renderSettings();
}

function renderHome() {
  const on = S.connected;
  $("powerBtn").classList.toggle("on", on);
  const cat = on ? "assets/cat-awake.png" : "assets/cat-sleep.png";
  if (!$("catImg").src.endsWith(cat)) { $("catImg").src = cat; $("brandCat").src = cat; }
  $("connText").textContent = on ? "已连接" : "未连接";
  $("tunTag").classList.toggle("hidden", !(on && S.tun.enabled && S.tun.privileged));
  let now = "";
  if (on && status) {
    if (status.node === "DIRECT") now = "全部直连";
    else if (status.node) {
      const d = status.delay != null ? ` · <span class="${delayClass(status.delay)}">${status.delay} ms</span>` : "";
      now = `${esc(status.region)} · ${esc(status.node)}${d}`;
    }
  } else if (!on) now = "小猫在睡觉，点它开始上网";
  $("nowText").innerHTML = now;

  const err = status?.error || S.error;
  $("homeError").classList.toggle("hidden", !err);
  $("homeError").textContent = err || "";

  $("defaultSelect").innerHTML = targetOptions(S.defaultTarget, "default");

  const subs = S.subscriptions;
  $("subInfo").textContent = `${subs.length} 个订阅 · 共 ${S.nodeCount} 个节点`;
  $("homeSubs").innerHTML = subs.map((sub) => {
    const u = sub.usage;
    const right = [];
    if (u && u.total) right.push(`已用 ${fmtBytes(u.upload + u.download)} / ${fmtBytes(u.total)}`);
    if (u && u.expire) right.push(`${fmtDate(u.expire * 1000)} 到期`);
    const bar = u && u.total
      ? `<div class="usage"><div style="width:${Math.min(100, ((u.upload + u.download) / u.total) * 100)}%"></div></div>` : "";
    return `<div class="home-sub"><div class="line"><span><b>${esc(sub.name)}</b> · ${sub.nodeCount} 个节点</span><span>${esc(right.join(" · "))}</span></div>${bar}</div>`;
  }).join("");
}

function subMeta(sub) {
  const parts = [`${sub.nodeCount} 个节点`, sub.url ? `更新于 ${fmtAgo(sub.updatedAt)}` : `导入于 ${fmtAgo(sub.updatedAt)}`];
  const u = sub.usage;
  if (u && u.total) parts.push(`已用 ${fmtBytes(u.upload + u.download)} / ${fmtBytes(u.total)}`);
  if (u && u.expire) parts.push(`${fmtDate(u.expire * 1000)} 到期`);
  return parts.join(" · ");
}

function renderSubs() {
  const subs = S.subscriptions;
  $("subsSummary").textContent = `${subs.length} 个订阅 · 共 ${S.nodeCount} 个节点`;
  $("subsList").innerHTML = subs.map((sub) => `<div class="sub-row">
      <div><div class="sub-name">${esc(sub.name)}${sub.url ? "" : '<span class="tag-file">文件</span>'}</div>
        <div class="sub-meta" title="${esc(sub.url || "从文件导入")}">${esc(subMeta(sub))}</div></div>
      <div class="sub-ops">
        ${sub.url ? `<button class="ghost" data-sop="refresh" data-sid="${esc(sub.id)}">更新</button>` : ""}
        <button class="ghost" data-sop="rename" data-sid="${esc(sub.id)}">改名</button>
        <button class="ghost danger" data-sop="del" data-sid="${esc(sub.id)}">删除</button>
      </div></div>`).join("");
}

const CAT_ORDER = ["AI", "视频", "社交", "常用", "科研", "国内", "我添加的"];
function renderSites() {
  const fb = S.fallbacks || [];
  $("fallbackWarn").classList.toggle("hidden", !fb.length);
  $("fallbackWarn").textContent = fb.length ? `订阅里已经没有这些选择对应的节点或地区，暂时改走默认地区：${fb.join("、")}` : "";

  const cats = {};
  for (const s of S.sites) (cats[s.cat] ||= []).push(s);
  const order = Object.keys(cats).sort((a, b) => (CAT_ORDER.indexOf(a) + 99) % 99 - (CAT_ORDER.indexOf(b) + 99) % 99);
  $("siteGroups").innerHTML = order
    .map((cat) => `<div class="cat-title">${esc(cat)}</div><div class="card site-card">${cats[cat]
      .map((s) => `<div class="site-row" data-id="${esc(s.id)}">
          <div><div class="site-name" data-edit="${esc(s.id)}" title="点击编辑包含的域名">${esc(s.name)}${s.edited ? '<span class="edited-tag">已改</span>' : ""}</div>
            <div class="site-sub">${esc(s.domains.join("  "))}</div></div>
          <select data-choice="${esc(s.id)}" class="${s.choice ? "set" : ""}">${targetOptions(s.choice, "site")}</select>
          <div class="site-actions"><button class="ghost small-btn" data-check="${esc(s.id)}">检查</button></div>
          <div class="site-check hidden" id="chk-${esc(s.id)}"></div>
        </div>`)
      .join("")}</div>`)
    .join("");

  // 自定义规则：只有在没编辑时才用后端数据刷新
  if (!$("rulesBox").dataset.dirty) rulesDraft = (S.customRules || []).map((r) => ({ ...r }));
  renderRules();

  $("siteList").innerHTML = S.sites.map((s) => `<option value="${esc(s.url)}">${esc(s.name)}</option>`).join("");
}

const RULE_KINDS = [
  ["suffix", "域名（含子域名）"],
  ["domain", "完整域名"],
  ["keyword", "域名关键词"],
  ["ipcidr", "IP / IP 段"],
];
function renderRules() {
  $("rulesList").innerHTML = rulesDraft.length
    ? rulesDraft.map((r, i) => `<div class="rule-row" data-i="${i}">
        <select data-k="kind">${RULE_KINDS.map(([v, l]) => `<option value="${v}"${v === r.kind ? " selected" : ""}>${l}</option>`).join("")}</select>
        <input data-k="value" value="${esc(r.value)}" placeholder="${r.kind === "ipcidr" ? "1.2.3.0/24" : r.kind === "keyword" ? "google" : "example.com"}" spellcheck="false" />
        <select data-k="target">${targetOptions(r.target || "default", "rule")}</select>
        <div class="ord"><button class="ghost" data-op="up" title="上移">↑</button><button class="ghost" data-op="down" title="下移">↓</button><button class="ghost danger" data-op="del" title="删除">✕</button></div>
      </div>`).join("")
    : `<p class="muted small">还没有自定义规则。</p>`;
}

function renderSettings() {
  if (document.activeElement !== $("mixedPort")) $("mixedPort").value = S.mixedPort;
  if (document.activeElement !== $("ctlPort")) $("ctlPort").value = S.controllerPort;
  $("dataDir").textContent = "数据目录：" + S.dataDir;
  renderSubs();
  renderTun();
  renderGroups();
  renderNodes();
}

function renderNodes() {
  $("nodesList").innerHTML = (S.regions || [])
    .map((r) => `<div class="region-block"><div class="region-name">${esc(r.name)} <span class="muted small">${r.nodes.length} 个</span></div>
      <div class="node-grid">${r.nodes.map((n) => {
        const d = delays[n];
        const txt = d == null ? (Object.keys(delays).length ? "超时" : "") : d + " ms";
        return `<div class="node-item"><span class="n" title="${esc(n)}">${esc(n)}</span><span class="${delayClass(d)}">${txt}</span></div>`;
      }).join("")}</div></div>`)
    .join("");
}

function renderGroups() {
  const gs = S.groups || [];
  $("groupsList").innerHTML = gs.length
    ? gs.map((g, i) => `<div class="group-row">
        <div><div class="group-title">${esc(g.name)} <span class="muted small">${g.matched.length} 个节点</span>${g.matched.length ? "" : '<span class="empty-tag">没有匹配的节点，暂不可选</span>'}</div>
          <div class="group-kw" title="${esc([...g.keywords, ...g.nodes].join("\n"))}">${esc(g.keywords.length ? "关键词：" + g.keywords.join("  ") : "")}${g.keywords.length && g.nodes.length ? " · " : ""}${g.nodes.length ? `手动选了 ${g.nodes.length} 个节点` : ""}</div></div>
        <div class="group-ops">
          <button class="ghost" data-gop="up" data-gid="${esc(g.id)}" ${i === 0 ? "disabled" : ""} title="上移">↑</button>
          <button class="ghost" data-gop="down" data-gid="${esc(g.id)}" ${i === gs.length - 1 ? "disabled" : ""} title="下移">↓</button>
          <button class="ghost" data-gop="edit" data-gid="${esc(g.id)}">编辑</button>
          <button class="ghost danger" data-gop="del" data-gid="${esc(g.id)}">删除</button>
        </div></div>`).join("")
    : `<p class="muted small">还没有分组，全部节点都在「其他」里。</p>`;
  $("groupsSummary").textContent = `${gs.length} 个分组`;
  const other = (S.regions || []).find((r) => !r.id);
  $("otherInfo").textContent = other ? `还有 ${other.nodes.length} 个节点不在任何分组，归入「其他」。` : "所有节点都已分组。";
}

function renderTun() {
  const t = S.tun;
  const active = t.enabled && t.privileged;
  $("tunToggle").checked = t.enabled;
  $("tunSummary").textContent = active ? "已开启" : t.enabled ? "未生效" : "关闭";
  let hint = "", fix = "";
  if (t.platform === "mac") {
    hint = "第一次开启会弹出系统密码框（输入开机密码），给代理内核授权。软件更新后需要再授权一次。";
    if (t.enabled && !t.privileged) { hint = "⚠️ 增强模式还没生效：代理内核需要重新授权（更新软件后会这样）。"; fix = "授权"; }
  } else if (t.platform === "windows") {
    hint = "需要以管理员身份运行 Clash for Young。开启后软件会以管理员身份重新启动（会弹出确认窗口）。";
    if (t.enabled && !t.privileged) { hint = "⚠️ 增强模式还没生效：需要以管理员身份运行。"; fix = "以管理员身份重新启动"; }
  } else {
    hint = "这个系统暂不支持增强模式。";
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
  $("welcomeMsg").textContent = "正在下载订阅…";
  try {
    S = await call("add_subscription", { url }, { quiet: true });
    $("welcomeMsg").textContent = `找到 ${S.nodeCount} 个节点，正在连接…`;
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
  $("connText").textContent = S.connected ? "正在断开…" : "正在连接…";
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
    toast("已切换默认地区");
  } catch (_) { render(); }
});

async function refreshSub(id) {
  toast("正在更新订阅…");
  try {
    S = await call("refresh_subscription", { id: id || null });
    render();
    toast(`订阅已更新：共 ${S.nodeCount} 个节点`);
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
  toast("正在下载订阅…");
  try {
    S = await call("add_subscription", { url });
    $("subUrl").value = "";
    render();
    toast(`已添加：现在共 ${S.nodeCount} 个节点`);
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
    toast(`已导入 ${ok} 个文件：现在共 ${S.nodeCount} 个节点`);
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
    const name = await askText(`给「${sub.name}」改个名字`, sub.name);
    if (!name) return;
    try { S = await call("rename_subscription", { id, name }); render(); } catch (_) {}
  }
  if (op === "del") {
    const last = S.subscriptions.length === 1;
    if (!(await askConfirm(`删除订阅「${sub.name}」？它的 ${sub.nodeCount} 个节点会被移除。${last ? "这是最后一个订阅，删除后会断开连接。" : ""}`))) return;
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
    toast("已生效");
  } catch (_) { renderSites(); }
});
$("siteGroups").addEventListener("click", async (e) => {
  const cid = e.target.dataset.check;
  const eid = e.target.closest("[data-edit]")?.dataset.edit;
  if (cid) {
    const site = S.sites.find((s) => s.id === cid);
    const box = $("chk-" + cid);
    box.classList.remove("hidden");
    box.innerHTML = `<span class="muted">正在检查…</span>`;
    try {
      const r = await call("check_url", { url: site.url }, { quiet: true });
      const d = r.nodeDelayMs != null ? ` · <span class="${delayClass(r.nodeDelayMs)}">${r.nodeDelayMs} ms</span>` : "";
      const via = r.node === "DIRECT" ? "直连" : `${esc(r.node)}`;
      box.innerHTML = `实际走 <b>${via}</b>${d} <span class="muted">（规则：${esc(r.rule)} ${esc(r.rulePayload)}）</span>`;
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
    toast("已添加到「我添加的」，可以给它选择走向");
  } catch (_) {}
});

// 编辑网站对话框
let editing = null;
function openSiteDialog(site) {
  editing = site;
  $("sdTitle").textContent = site.custom ? "编辑网站" : `编辑「${site.name}」的域名`;
  $("sdName").value = site.name;
  $("sdUrl").value = site.url;
  $("sdName").disabled = $("sdUrl").disabled = !site.custom;
  $("sdDomains").value = site.domains.join("\n");
  $("sdDelete").textContent = site.custom ? "删除网站" : "恢复默认域名";
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
    toast("已保存");
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
    toast("规则已保存并生效");
  } catch (_) {}
});

// 设置页：分流检查
$("checkForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const url = $("checkUrl").value.trim();
  if (!url) return;
  $("checkBtn").disabled = true;
  $("checkResult").innerHTML = `<p class="muted">正在经代理访问并读取分流信息…</p>`;
  try {
    const r = await call("check_url", { url }, { quiet: true });
    const chain = r.chains.map((c) => `<span class="chip">${esc(c)}</span>`).join(`<span class="arrow">←</span>`);
    const exit = r.exitIp ? `${esc(r.exitIp.ip)} <span class="muted">${esc(r.exitIp.location)} ${esc(r.exitIp.org)}</span>` : esc(r.exitIpError || "");
    $("checkResult").innerHTML = `<div class="check-res">
      ${r.warnings.map((w) => `<div class="warn">${esc(w)}</div>`).join("")}
      <div class="kv"><span class="k">节点</span><b>${esc(r.node === "DIRECT" ? "直连" : r.node)}</b></div>
      <div class="kv"><span class="k">链路</span><div class="chain">${chain}</div></div>
      <div class="kv"><span class="k">命中规则</span><span class="mono">${esc(r.rule)} ${esc(r.rulePayload)}</span></div>
      <div class="kv"><span class="k">节点延迟</span><span class="${delayClass(r.nodeDelayMs)}">${r.nodeDelayMs != null ? r.nodeDelayMs + " ms" : esc(r.nodeDelayError || "-")}</span></div>
      <div class="kv"><span class="k">本次请求</span><span>${r.requestMs} ms${r.httpStatus ? " · HTTP " + r.httpStatus : ""}</span></div>
      <div class="kv"><span class="k">出口 IP</span><span class="mono">${exit}</span></div>
    </div>`;
  } catch (err) {
    $("checkResult").innerHTML = `<div class="error">${esc(err)}</div>`;
  } finally {
    $("checkBtn").disabled = false;
  }
});

$("delayBtn").addEventListener("click", async () => {
  $("delayBtn").disabled = true;
  $("delayMsg").textContent = "测试中，最多 30 秒…";
  try {
    delays = (await call("node_delays")) || {};
    $("delayMsg").textContent = `${Object.keys(delays).length} 个节点可用`;
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
    render(); toast("端口已保存");
  } catch (_) {}
});
$("logBtn").addEventListener("click", async () => {
  const box = $("logBox");
  box.textContent = (await call("core_log")) || "（暂无日志）";
  box.classList.remove("hidden");
});
$("cfgBtn").addEventListener("click", async () => {
  const box = $("logBox");
  box.textContent = (await call("current_config")) || "（还没有生成配置）";
  box.classList.remove("hidden");
});

// 增强模式
async function setTun(enabled) {
  $("tunToggle").disabled = true;
  try {
    if (enabled) toast(S.tun.platform === "mac" ? "请在弹出的窗口里输入开机密码…" : "正在开启增强模式…");
    S = await call("set_tun", { enabled }, { quiet: true });
    toast(enabled ? "增强模式已开启" : "增强模式已关闭");
  } catch (err) {
    if (String(err) === "NEED_ADMIN") {
      await refreshState();
      if (await askConfirm("增强模式需要以管理员身份运行。现在以管理员身份重新启动 Clash for Young 吗？（会弹出系统确认窗口）")) {
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
    $("exportMobileInfo").textContent = "已保存到「下载」文件夹：" + path.split(/[\\/]/).pop();
    toast("已导出，文件在「下载」文件夹");
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
        <span>${esc(n)}</span>${byKw ? '<span class="why">关键词</span>' : ""}</label>`;
    }).join("") || `<span class="muted small">没有节点</span>`;
  $("gdCount").textContent = `共 ${gdMatched.size} 个节点`;
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
  $("gdTitle").textContent = g ? `编辑分组「${g.name}」` : "新建分组";
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
    toast("分组已保存并生效");
  } catch (_) {}
});
$("groupsList").addEventListener("click", async (e) => {
  const op = e.target.dataset.gop, id = e.target.dataset.gid;
  if (!op) return;
  const g = S.groups.find((x) => x.id === id);
  try {
    if (op === "edit") return openGroupDialog(g);
    if (op === "del") {
      if (!(await askConfirm(`删除分组「${g.name}」？选了这个分组的网站会改走默认地区。`))) return;
      S = await call("delete_group", { id });
    }
    if (op === "up" || op === "down") S = await call("move_group", { id, delta: op === "up" ? -1 : 1 });
    render();
  } catch (_) {}
});
$("resetGroupsBtn").addEventListener("click", async () => {
  if (!(await askConfirm("恢复成默认的「美国」「台湾」两个分组？你新建的分组会被删除。"))) return;
  try { S = await call("reset_groups"); render(); toast("已恢复默认分组"); } catch (_) {}
});

// ───────── 启动 ─────────
(async () => {
  await refreshState();
  if (S && S.subscriptions.length) show("home");
  render();
  pollStatus();
  setInterval(pollStatus, 5000);
  setInterval(refreshState, 60000);
})();
