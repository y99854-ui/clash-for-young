// 下载打包需要的 mihomo 内核和地理数据。
// 用法：node scripts/fetch-deps.mjs [--target <rust 目标三元组>]
//   不带参数时按当前电脑；Mac 通用版用 --target universal-apple-darwin
import { execSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import zlib from "node:zlib";

const MIHOMO_VERSION = process.env.MIHOMO_VERSION || "v1.19.32";
const root = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1")), "..");
const binDir = path.join(root, "src-tauri", "binaries");
const geoDir = path.join(root, "src-tauri", "resources", "geo");

function hostTarget() {
  const arch = os.arch() === "arm64" ? "aarch64" : "x86_64";
  if (process.platform === "darwin") return `${arch}-apple-darwin`;
  if (process.platform === "win32") return `${arch}-pc-windows-msvc`;
  return `${arch}-unknown-linux-gnu`;
}
const ti = process.argv.indexOf("--target");
const target = ti > 0 ? process.argv[ti + 1] : hostTarget();

const headers = { "User-Agent": "clash-for-young-build" };
if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;

async function get(url) {
  const r = await fetch(url, { headers });
  if (!r.ok) throw new Error(`${r.status} ${url}`);
  return Buffer.from(await r.arrayBuffer());
}

let assets;
async function asset(patterns) {
  assets ||= JSON.parse(
    (await get(`https://api.github.com/repos/MetaCubeX/mihomo/releases/tags/${MIHOMO_VERSION}`)).toString()
  ).assets;
  for (const p of patterns) {
    const a = assets.find((x) => p.test(x.name));
    if (a) return a;
  }
  throw new Error("找不到 mihomo 安装包：" + patterns.join(" / ") + "\n可用：" + assets.map((a) => a.name).join(", "));
}

// 返回解压后的可执行文件内容
async function mihomoFor(osName, arch) {
  const v = "v[\\d.]+";
  const ext = osName === "windows" ? "zip" : "gz";
  const pats =
    arch === "amd64"
      ? [
          new RegExp(`^mihomo-${osName}-amd64-v1-${v}\\.${ext}$`),
          new RegExp(`^mihomo-${osName}-amd64-compatible-${v}\\.${ext}$`),
          new RegExp(`^mihomo-${osName}-amd64-${v}\\.${ext}$`),
        ]
      : [new RegExp(`^mihomo-${osName}-${arch}-${v}\\.${ext}$`)];
  const a = await asset(pats);
  console.log("下载", a.name);
  const data = await get(a.browser_download_url);
  if (ext === "gz") return zlib.gunzipSync(data);
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "mihomo-"));
  fs.writeFileSync(path.join(tmp, "m.zip"), data);
  execSync("tar -xf m.zip", { cwd: tmp });
  const exe = fs.readdirSync(tmp).find((f) => f.endsWith(".exe"));
  return fs.readFileSync(path.join(tmp, exe));
}

fs.mkdirSync(binDir, { recursive: true });
fs.mkdirSync(geoDir, { recursive: true });

const out = path.join(binDir, `mihomo-${target}${target.includes("windows") ? ".exe" : ""}`);
if (target === "universal-apple-darwin") {
  const a = path.join(binDir, "mihomo-aarch64-apple-darwin");
  const b = path.join(binDir, "mihomo-x86_64-apple-darwin");
  fs.writeFileSync(a, await mihomoFor("darwin", "arm64"));
  fs.writeFileSync(b, await mihomoFor("darwin", "amd64"));
  execSync(`lipo -create -output "${out}" "${a}" "${b}"`);
} else {
  const osName = target.includes("apple") ? "darwin" : target.includes("windows") ? "windows" : "linux";
  const arch = target.startsWith("aarch64") ? "arm64" : "amd64";
  fs.writeFileSync(out, await mihomoFor(osName, arch));
}
fs.chmodSync(out, 0o755);
console.log("内核 →", out);

const GEO = [
  ["GeoSite.dat", "geosite.dat"],
  ["geoip.metadb", "country-lite.mmdb"],
];
for (const [name, remote] of GEO) {
  const dst = path.join(geoDir, name);
  if (fs.existsSync(dst)) continue;
  console.log("下载", remote);
  fs.writeFileSync(dst, await get(`https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/${remote}`));
}
console.log("地理数据 →", geoDir);
