// 打包 / 运行科研版 Clash for Research：和 Young 用同一套代码，
// 只是打包时设置 CFY_EDITION=research，并换上 research.conf.json 里的名字、标识、图标和更新地址。
// 用法：npm run dev:research（本地运行） / npm run build:research（打包），后面可以接 tauri 的其他参数。
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";

const [mode, ...rest] = process.argv.slice(2);
if (mode !== "dev" && mode !== "build") {
  console.error("用法：node scripts/research.mjs dev|build [tauri 参数]");
  process.exit(1);
}
const env = { ...process.env, CFY_EDITION: "research" };
const run = (args) => {
  const r = spawnSync("npx", ["tauri", ...args], { stdio: "inherit", env, shell: process.platform === "win32" });
  if (r.status !== 0) process.exit(r.status ?? 1);
};
// 绿色图标的各种尺寸（第一次运行时生成）
if (!existsSync("src-tauri/icons-research/icon.icns")) {
  run(["icon", "src-tauri/icons-research/app-icon.png", "-o", "src-tauri/icons-research"]);
}
run([mode, "--config", "src-tauri/research.conf.json", ...rest]);
