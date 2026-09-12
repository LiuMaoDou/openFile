import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { rustEnvironment } from "./runtime.mjs";
const child = spawn(
  process.execPath,
  [
    resolve(import.meta.dirname, "../node_modules/@tauri-apps/cli/tauri.js"),
    ...process.argv.slice(2),
  ],
  { stdio: "inherit", env: rustEnvironment() },
);
child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
child.on("exit", (code) => process.exit(code ?? 1));
