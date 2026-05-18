// Tauri sidecar 准备脚本
//
// 把 cargo 产出的 miniterm-hook 复制成 <name>-<target-triple> 形式，
// 放到 src-tauri/binaries/，供 tauri.conf.json 的 bundle.externalBin 识别。
//
// 触发时机: tauri.conf.json beforeBuildCommand（即 `tauri build` 时）。
//
// 设计原则：
// - 仅在 release 模式准备（`tauri build` 一定是 release）
// - 默认走当前 host triple；CI 跨平台时可通过 TAURI_TARGET_TRIPLE 环境变量覆盖
// - 若 miniterm-hook 还没编出来，主动 cargo build 一次，避免用户记错命令顺序

import { execSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, chmodSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const SRC_TAURI = resolve(__dirname, "..");

function getHostTriple() {
  if (process.env.TAURI_TARGET_TRIPLE) return process.env.TAURI_TARGET_TRIPLE;
  const out = execSync("rustc -vV", { encoding: "utf8" });
  const m = out.match(/^host:\s*(.+)$/m);
  if (!m) throw new Error("无法从 rustc -vV 解析 host triple");
  return m[1].trim();
}

function ensureHookBuilt(profile) {
  const exe = process.platform === "win32" ? "miniterm-hook.exe" : "miniterm-hook";
  // cargo 跑在 SRC_TAURI 工作目录，target 产物在 src-tauri/target/<profile>/。
  // 不要写成 SRC_TAURI/../target —— 那是 workspace root 风格的目录，本项目没有。
  const src = resolve(SRC_TAURI, "target", profile, exe);
  if (existsSync(src)) return src;
  console.log(`[prepare-sidecar] ${src} 不存在，先 cargo build --bin miniterm-hook --${profile}`);
  const args = ["build", "--bin", "miniterm-hook"];
  if (profile === "release") args.push("--release");
  const r = spawnSync("cargo", args, { cwd: SRC_TAURI, stdio: "inherit" });
  if (r.status !== 0) throw new Error("cargo build miniterm-hook 失败");
  if (!existsSync(src)) throw new Error(`cargo 已编完但仍找不到 ${src}`);
  return src;
}

function main() {
  // 优先 CLI 参数 (debug/release)，否则环境变量 PROFILE，最后默认 release
  const cli = process.argv[2];
  const profile = cli && (cli === "debug" || cli === "release")
    ? cli
    : (process.env.PROFILE === "debug" ? "debug" : "release");
  const triple = getHostTriple();
  const exeSuffix = process.platform === "win32" ? ".exe" : "";

  const dstDir = resolve(SRC_TAURI, "binaries");
  mkdirSync(dstDir, { recursive: true });
  const dst = resolve(dstDir, `miniterm-hook-${triple}${exeSuffix}`);

  // 解决循环依赖：tauri-build 在 lib build.rs 阶段校验 externalBin 资源存在，
  // 而该资源由 `cargo build --bin miniterm-hook` 产出 —— 后者又依赖 lib build.rs。
  // 解法：首次先放一个 0 字节占位文件让校验通过，编完后再用真 binary 覆盖。
  if (!existsSync(dst)) {
    writeFileSync(dst, "");
    if (process.platform !== "win32") chmodSync(dst, 0o755);
    console.log(`[prepare-sidecar] 创建占位文件 ${dst}（首次构建用）`);
  }

  const src = ensureHookBuilt(profile);

  copyFileSync(src, dst);
  if (process.platform !== "win32") chmodSync(dst, 0o755);
  console.log(`[prepare-sidecar] ${dst}`);
}

main();
