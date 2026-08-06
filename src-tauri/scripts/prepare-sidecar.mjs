// Tauri sidecar 准备脚本
//
// 把 cargo 产出的 miniterm-hook 复制成 <name>-<target-triple> 形式，
// 放到 src-tauri/binaries/，供 tauri.conf.json 的 bundle.externalBin 识别。
//
// 触发时机: tauri.conf.json beforeBuildCommand / beforeDevCommand。
//
// 三种典型场景：
// 1) 本地 dev（mac aarch64-only）：只生成 host triple 一个文件
// 2) CI Linux/Windows：只生成对应单 target 文件
// 3) CI macOS universal：rustup 安装了 aarch64 + x86_64 双 target，
//    本脚本会分别编译两个 arch，再用 `lipo -create` 合并为一个 universal
//    binary 文件名 `miniterm-hook-universal-apple-darwin`（Tauri build
//    --target universal-apple-darwin 阶段实际需要的就是这一个 fat binary）。
//
// 同时还为单 arch target 文件保留单独生成，以便：
// - 单架构 build（比如 --target aarch64-apple-darwin）也能找到对应 sidecar
// - tauri-build 在 lib build.rs 阶段对每个 cargo target 做的资源校验都能通过

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

function getInstalledTargets() {
  try {
    const out = execSync("rustup target list --installed", { encoding: "utf8" });
    return out.split("\n").map((s) => s.trim()).filter(Boolean);
  } catch {
    return [];
  }
}

/// 返回需要单独编译产出的 target triple 列表（不含 universal-apple-darwin 这种伪 triple）。
function resolveSingleArchTargets(hostTriple) {
  // 命令行覆盖（逗号分隔）；CI 可以传 --targets aarch64-apple-darwin,x86_64-apple-darwin
  const cliIdx = process.argv.findIndex((a) => a === "--targets");
  if (cliIdx >= 0 && process.argv[cliIdx + 1]) {
    return process.argv[cliIdx + 1].split(",").map((s) => s.trim()).filter(Boolean);
  }
  if (process.env.SIDECAR_TARGETS) {
    return process.env.SIDECAR_TARGETS.split(",").map((s) => s.trim()).filter(Boolean);
  }

  // macOS universal 自动检测：rustup 同时装了 aarch64 和 x86_64 → 双 arch 编译
  if (process.platform === "darwin") {
    const installed = getInstalledTargets();
    if (installed.includes("aarch64-apple-darwin") &&
        installed.includes("x86_64-apple-darwin")) {
      return ["aarch64-apple-darwin", "x86_64-apple-darwin"];
    }
  }

  return [hostTriple];
}

function buildHookForTarget(profile, target, isHostTriple) {
  const exe = process.platform === "win32" ? "miniterm-hook.exe" : "miniterm-hook";
  // 优先 target 子目录路径（CI 风格），fallback 到 flat（host triple 默认）
  const tripleDir = resolve(SRC_TAURI, "target", target, profile, exe);
  const flatDir = resolve(SRC_TAURI, "target", profile, exe);

  const pickExistingSrc = () => {
    if (existsSync(tripleDir)) return tripleDir;
    if (isHostTriple && existsSync(flatDir)) return flatDir;
    return null;
  };

  // 即使已有产物也必须执行 cargo build。Cargo 会使用增量缓存，成本很低；
  // 直接复用文件会在 helper 源码变化后把旧二进制继续带入 dev/安装包。
  console.log(`[prepare-sidecar] cargo build --bin miniterm-hook --target ${target} (${profile})`);
  const args = ["build", "--bin", "miniterm-hook", "--target", target];
  if (profile === "release") args.push("--release");
  const r = spawnSync("cargo", args, { cwd: SRC_TAURI, stdio: "inherit" });
  if (r.status !== 0) throw new Error(`cargo build miniterm-hook (${target}) 失败`);

  const src = pickExistingSrc();
  if (!src) throw new Error(`cargo 已编完但 ${tripleDir} 和 ${flatDir} 都找不到`);
  return src;
}

function prepareSingleArchSidecar(profile, target, hostTriple) {
  const exeSuffix = target.includes("windows") ? ".exe" : "";
  const dstDir = resolve(SRC_TAURI, "binaries");
  mkdirSync(dstDir, { recursive: true });
  const dst = resolve(dstDir, `miniterm-hook-${target}${exeSuffix}`);

  // 先放 0 字节占位让 tauri-build 校验通过
  if (!existsSync(dst)) {
    writeFileSync(dst, "");
    if (!target.includes("windows")) chmodSync(dst, 0o755);
    console.log(`[prepare-sidecar] 创建占位 ${dst}`);
  }

  const src = buildHookForTarget(profile, target, target === hostTriple);
  copyFileSync(src, dst);
  if (!target.includes("windows")) chmodSync(dst, 0o755);
  console.log(`[prepare-sidecar] ${dst}`);
  return dst;
}

/// 用 lipo 把 aarch64 + x86_64 两个 darwin binary 合并成 universal fat binary。
///
/// 同时写两个位置：
/// 1. `binaries/miniterm-hook-universal-apple-darwin`
///    匹配 externalBin sidecar 命名约定，让 tauri-build lib 阶段校验通过。
/// 2. `target/universal-apple-darwin/release/miniterm-hook`
///    匹配 cargo 默认 [[bin]] 产物路径风格。Tauri v2 bundling 阶段对 universal
///    target 实际找的就是这个路径（实测错误信息：`Failed to copy binary from
///    target/universal-apple-darwin/release/miniterm-hook ... does not exist`），
///    cargo 不会自动 lipo 合并 [[bin]]，必须手动生成。
function prepareUniversalSidecar(profile, aarch64Path, x86_64Path) {
  const dstBin = resolve(SRC_TAURI, "binaries", "miniterm-hook-universal-apple-darwin");
  const cargoUniDir = resolve(SRC_TAURI, "target", "universal-apple-darwin", profile);
  const dstCargo = resolve(cargoUniDir, "miniterm-hook");
  mkdirSync(cargoUniDir, { recursive: true });

  const runLipo = (out) => {
    console.log(`[prepare-sidecar] lipo -create -> ${out}`);
    const r = spawnSync("lipo", ["-create", "-output", out, aarch64Path, x86_64Path], {
      stdio: "inherit",
    });
    if (r.status !== 0) throw new Error(`lipo -create 失败: ${out}`);
    chmodSync(out, 0o755);
  };

  runLipo(dstBin);
  runLipo(dstCargo);
}

function main() {
  const positional = process.argv.slice(2).filter((a) => !a.startsWith("--") && (a === "debug" || a === "release"));
  const profile = positional[0] || (process.env.PROFILE === "debug" ? "debug" : "release");

  const hostTriple = getHostTriple();
  const singleArchTargets = resolveSingleArchTargets(hostTriple);
  console.log(`[prepare-sidecar] profile=${profile} targets=${singleArchTargets.join(",")}`);

  const produced = {};
  for (const target of singleArchTargets) {
    produced[target] = prepareSingleArchSidecar(profile, target, hostTriple);
  }

  // macOS 双 arch 同时存在 → 合并出 universal fat binary（写两个位置）
  if (produced["aarch64-apple-darwin"] && produced["x86_64-apple-darwin"]) {
    prepareUniversalSidecar(
      profile,
      produced["aarch64-apple-darwin"],
      produced["x86_64-apple-darwin"],
    );
  }
}

main();
