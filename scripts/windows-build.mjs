import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, resolve } from "node:path";
import { rustEnvironment } from "./runtime.mjs";

const root = resolve(import.meta.dirname, "..");
const cross = process.platform !== "win32";
const env = rustEnvironment();
const candidates = [
  resolve(root, ".tools/windows-build/bin"),
  "/usr/local/opt/llvm@20/bin",
  "/opt/homebrew/opt/llvm@20/bin",
  "/usr/local/opt/lld@20/bin",
  "/opt/homebrew/opt/lld@20/bin",
  "/usr/local/opt/llvm/bin",
  "/opt/homebrew/opt/llvm/bin",
  "/usr/local/opt/lld/bin",
  "/opt/homebrew/opt/lld/bin",
].filter(existsSync);
if (cross) {
  env.PATH = [...candidates, env.PATH].join(delimiter);
  env.XWIN_CACHE_DIR ||= resolve(root, ".tools/xwin");
  env.XWIN_ARCH = "x86_64";
}
env.CARGO_BUILD_JOBS ||= "2";
// .cargo/config.toml already links the complete CRT statically. Tauri's
// selective VC-runtime override forces dynamic UCRT and conflicts with xwin.
env.STATIC_VCRUNTIME = "false";

const child = spawn(
  process.execPath,
  [
    resolve(root, "node_modules/@tauri-apps/cli/tauri.js"),
    "build",
    "--target",
    "x86_64-pc-windows-msvc",
    ...(cross ? ["--runner", "cargo-xwin"] : []),
    ...process.argv.slice(2),
  ],
  { cwd: root, env, stdio: "inherit" },
);
child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
child.on("exit", (code) => process.exit(code ?? 1));
