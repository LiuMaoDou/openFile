import { spawn } from "node:child_process";
import { rustEnvironment } from "./runtime.mjs";
const child = spawn("cargo", process.argv.slice(2), {
  stdio: "inherit",
  env: rustEnvironment(),
});
child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
child.on("exit", (code) => process.exit(code ?? 1));
