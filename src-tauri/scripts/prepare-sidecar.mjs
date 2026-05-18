// Tauri sidecar 准备脚本
//
// 把 cargo 产出的 miniterm-hook 复制成 <name>-<target-triple> 形式，
// 放到 src-tauri/binaries/，供 tauri.conf.json 的 bundle.externalBin 识别。
//
// 触发时机: tauri.conf.json beforeBuildCommand / beforeDevCommand。
//
// 设计原则：
// - profile 通过命令行参数 (debug/release) 指定，默认 release
// - 默认按当前 host triple 生成；macOS 上若 toolchain 同时安装了 aarch64 和
//   x86_64 两个 target（CI universal build 必备），自动同时生成两个 sidecar，
//   让 `tauri build --target universal-apple-darwin` 能找到全部资源
// - 若 miniterm-hook 还没编出来，主动 cargo build 一次
// - 解决 tauri-build 校验 externalBin 资源 ↔ cargo build 产出 sidecar 的循环
//   依赖：先放 0 字节占位文件让 build.rs 校验通过，cargo build 完后真 binary
//   覆盖占位

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

/// 返回 rustup 已安装的 target triple 列表
function getInstalledTargets() {
  try {
    const out = execSync("rustup target list --installed", { encoding: "utf8" });
    return out.split("\n").map((s) => s.trim()).filter(Boolean);
  } catch {
    return [];
  }
}

/// 决定本次需要生成 sidecar 的 target 列表
function resolveTargets(hostTriple) {
  // 命令行覆盖（逗号分隔）；CI 可以传 --targets aarch64-apple-darwin,x86_64-apple-darwin
  const cliIdx = process.argv.findIndex((a) => a === "--targets");
  if (cliIdx >= 0 && process.argv[cliIdx + 1]) {
    return process.argv[cliIdx + 1].split(",").map((s) => s.trim()).filter(Boolean);
  }
  if (process.env.SIDECAR_TARGETS) {
    return process.env.SIDECAR_TARGETS.split(",").map((s) => s.trim()).filter(Boolean);
  }

  // macOS universal 自动检测：rustup 同时装了 aarch64 和 x86_64 → 双生成
  if (process.platform === "darwin") {
    const installed = getInstalledTargets();
    const macTargets = installed.filter((t) => t.endsWith("-apple-darwin"));
    if (macTargets.includes("aarch64-apple-darwin") &&
        macTargets.includes("x86_64-apple-darwin")) {
      return ["aarch64-apple-darwin", "x86_64-apple-darwin"];
    }
  }

  return [hostTriple];
}

function buildHookForTarget(profile, target, isHostTriple) {
  const exe = process.platform === "win32" ? "miniterm-hook.exe" : "miniterm-hook";
  // 非 host triple 用 cargo --target，产物路径是 target/<triple>/<profile>/。
  // host triple 不传 --target，产物在 target/<profile>/（cargo 默认行为）。
  // 注意：如果用户给所有 build 都加了 --target host-triple（CI 普遍写法），
  //       cargo 产物会落到 target/<triple>/<profile>/，不在 target/<profile>/。
  //       下面探测两个路径，存在哪个用哪个。
  const tripleDir = resolve(SRC_TAURI, "target", target, profile, exe);
  const flatDir = resolve(SRC_TAURI, "target", profile, exe);

  // 选择 src 路径：优先 triple 子目录（CI 风格），fallback 到 flat（本地默认风格）
  const pickExistingSrc = () => {
    if (existsSync(tripleDir)) return tripleDir;
    if (isHostTriple && existsSync(flatDir)) return flatDir;
    return null;
  };

  let src = pickExistingSrc();
  if (src) return src;

  console.log(`[prepare-sidecar] cargo build --bin miniterm-hook --target ${target} (${profile})`);
  const args = ["build", "--bin", "miniterm-hook", "--target", target];
  if (profile === "release") args.push("--release");
  const r = spawnSync("cargo", args, { cwd: SRC_TAURI, stdio: "inherit" });
  if (r.status !== 0) throw new Error(`cargo build miniterm-hook (${target}) 失败`);

  src = pickExistingSrc();
  if (!src) throw new Error(`cargo 已编完但 ${tripleDir} 和 ${flatDir} 都找不到`);
  return src;
}

function prepareSidecarForTarget(profile, target, hostTriple) {
  const exeSuffix = target.includes("windows") ? ".exe" : "";
  const dstDir = resolve(SRC_TAURI, "binaries");
  mkdirSync(dstDir, { recursive: true });
  const dst = resolve(dstDir, `miniterm-hook-${target}${exeSuffix}`);

  // 先放 0 字节占位让 tauri-build 校验通过（详见模块顶注释）
  if (!existsSync(dst)) {
    writeFileSync(dst, "");
    if (!target.includes("windows")) chmodSync(dst, 0o755);
    console.log(`[prepare-sidecar] 创建占位 ${dst}`);
  }

  const src = buildHookForTarget(profile, target, target === hostTriple);
  copyFileSync(src, dst);
  if (!target.includes("windows")) chmodSync(dst, 0o755);
  console.log(`[prepare-sidecar] ${dst}`);
}

function main() {
  // profile：CLI 第一个非 flag 参数（debug/release），否则环境变量 PROFILE，最后默认 release
  const positional = process.argv.slice(2).filter((a) => !a.startsWith("--") && (a === "debug" || a === "release"));
  const profile = positional[0] || (process.env.PROFILE === "debug" ? "debug" : "release");

  const hostTriple = getHostTriple();
  const targets = resolveTargets(hostTriple);
  console.log(`[prepare-sidecar] profile=${profile} targets=${targets.join(",")}`);

  for (const target of targets) {
    prepareSidecarForTarget(profile, target, hostTriple);
  }
}

main();
