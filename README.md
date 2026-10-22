# Clash for Young

一个新手也能自己装好、用好的代理客户端：**粘贴订阅、点连接、给网站选地区**。

基于 [mihomo](https://github.com/MetaCubeX/mihomo) 内核，支持 Mac 和 Windows，只支持 Clash 格式订阅。

## 功能（第一版）

- **第一次打开只问一件事**：粘贴订阅链接（或从文件导入），自动下载、按分组归类节点，然后连接
- **多个订阅**：可以添加多个订阅链接，也可以导入 Clash / mihomo 的 .yaml 文件；所有订阅的节点合在一起使用、一起分组
- **节点分组**：默认有「美国」「台湾」两个分组；可以自己新建、编辑、删除、排序分组，按节点名关键词自动归入，也可以手动勾选节点；没进任何分组的节点归入「其他」
- **小猫状态**：小猫睡着 = 未连接，睁眼 = 已连接。首页的开关、菜单栏 / 任务栏托盘图标、窗口图标都会跟着变；托盘菜单里可以直接连接、断开、退出
- **增强模式（TUN）**：可选，接管所有程序的网络（终端、游戏等不认系统代理的软件）。Mac 第一次开启输入一次开机密码给内核授权（软件更新后需再授权）；Windows 需以管理员身份运行
- **后台运行**：点窗口的关闭按钮会收起到 Mac 菜单栏（程序坞图标也隐藏）/ Windows 任务栏右下角托盘，代理继续工作；点小猫重新打开，要完全退出用小猫菜单里的「退出」或 Cmd+Q
- **首页**：点小猫连接 / 断开；当前分组、节点和延迟；默认地区（自动最快 / 某个分组 / 某个节点 / 全部直连）；订阅流量和到期时间
- **网站**：常用网站（AI、视频、社交、常用、科研、国内）每个都能单独选 自动 / 直连 / 任一分组 / 任一具体节点，改完立即生效，可一键检查实际走了哪个节点
  - 每个网站自动包含它的相关域名（例如 Discord 包含 discordapp.net 等 CDN 域名），可以编辑
  - 可以添加自己的网站
  - 高级：自定义规则，按域名 / 关键词 / IP 段指定走向，可调整顺序
- **不自动连接**：打开软件后由你点小猫连接
- **DNS 不泄漏**：国内网站用国内 DNS，其他网站的 DNS 经代理用国外 DNS 解析；增强模式用 fake-ip
- **手机**：设置里一键导出手机配置（分组、网站选择、自定义规则和电脑一致，订阅节点在手机上自动更新），导入 iPhone 的 Clash Mi 或安卓的 Clash Meta for Android / FlClash
- **设置**：订阅和节点分组（折叠显示）；链接订阅每 6 小时自动更新；高级里有分流检查、节点列表和测速、端口、内核日志、生成的配置
- 国内网站和内网始终直连；端口被占用时自动换端口；退出时恢复系统代理，崩溃后下次启动也会恢复；内核异常退出自动重启

## 下载安装

在 [Releases](../../releases) 下载：

| 系统 | 文件 |
|---|---|
| Mac（Intel 和 Apple 芯片通用） | `clash-for-young_版本号_universal.dmg` |
| Windows | `clash-for-young_版本号_x64-setup.exe` |

**Mac**：打开 `.dmg`，把 Clash for Young 拖进「应用程序」。没有苹果开发者签名，第一次打开会被拦截：

- 提示“无法验证开发者”：到「系统设置 → 隐私与安全性」点「仍要打开」
- 提示“已损坏”：在终端执行 `xattr -dr com.apple.quarantine "/Applications/Clash for Young.app"`

**Windows**：运行安装程序；出现 SmartScreen 提示点「更多信息 → 仍要运行」。
如果杀毒软件提示 `mihomo.exe` 有风险（代理内核常被误报），请在杀毒软件里恢复它并把安装目录加入白名单。

## 万一断网

软件正常退出时会自动关闭系统代理。如果软件被强制结束后上不了网，重新打开 Clash for Young 会自动恢复；或者手动关闭系统代理：

- Mac：系统设置 → 网络 → 当前网络 → 详细信息 → 代理，关掉所有勾选
- Windows：设置 → 网络和 Internet → 代理 → 关闭「使用代理服务器」

## 分流规则

从上到下依次匹配：

1. 自定义规则（高级）
2. 网站页里单独设置过的网站
3. 内网地址、国内网站（GeoSite cn / GeoIP CN）→ 直连
4. 其余 → 首页的默认地区

订阅里自带的规则不使用，只用它的节点。

## 从源码运行

需要 [Rust](https://rustup.rs) 和 [Node.js](https://nodejs.org)（Mac 还需要 Xcode 命令行工具）。

```bash
npm install
npm run deps     # 下载 mihomo 内核和地理数据（需要能访问 GitHub）
npm run dev      # 开发模式
npm run build    # 打包
```

Mac 通用版：`node scripts/fetch-deps.mjs --target universal-apple-darwin && npm run tauri build -- --target universal-apple-darwin`

测试：

```bash
cd src-tauri
cargo test                          # 单元测试
# 端到端测试：需要真实 mihomo 和一个本地订阅服务
CFY_MIHOMO=/path/to/mihomo CFY_SUB_URL=http://127.0.0.1:18080/sub CFY_GEO=resources/geo cargo test -- --ignored
```

推送 `v` 开头的标签，GitHub Actions 会自动打包 Mac 和 Windows 安装包并附到发布页。

## 项目结构

```
src/                    界面（普通网页）
  index.html  style.css  main.js
src-tauri/src/
  main.rs               入口、启动和退出处理、定时更新订阅
  app.rs                软件状态和界面调用的全部命令
  subscription.rs       下载、解析订阅，按节点名分地区
  configgen.rs          生成 mihomo 配置（策略组和规则）
  mobile.rs             导出手机配置
  core.rs               启动/停止/自动重启内核，重新加载配置
  sysproxy_ctl.rs       设置和恢复系统代理
  check.rs              分流检查（沿用「分流检查」工具）
  sites.rs / sites.json 内置网站和域名组
  settings.rs           用户设置
scripts/fetch-deps.mjs  下载内核和地理数据
```

数据保存在：Mac `~/Library/Application Support/com.joe.clash-for-young/`，Windows `%APPDATA%\com.joe.clash-for-young\`。

## 许可证

GPL-3.0。mihomo 内核同为 GPL-3.0。
