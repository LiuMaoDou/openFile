import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { resolve } from "node:path";
import { rustEnvironment } from "./runtime.mjs";
const env = {
  ...rustEnvironment(),
  FILEM_DEV_TOKEN: randomBytes(32).toString("hex"),
  FILEM_DATA_DIR: process.env.FILEM_DATA_DIR || resolve(".filem"),
};
const children = [];
let stopping = false;
function stop(code = 0) {
  if (stopping) return;
  stopping = true;
  children.forEach((child) => child.kill());
  setTimeout(() => process.exit(code), 300).unref();
}
process.on("SIGINT", () => stop());
process.on("SIGTERM", () => stop());
function start(command, args) {
  const child = spawn(command, args, { env, stdio: "inherit" });
  children.push(child);
  child.on("error", (error) => {
    console.error(error.message);
    stop(1);
  });
  child.on("exit", (code) => {
    if (!stopping) stop(code ?? 1);
  });
  return child;
}
start("cargo", ["run", "-p", "filem-dev"]);
let ready = false;
for (let i = 0; i < 900 && !stopping; i++) {
  try {
    const res = await fetch("http://127.0.0.1:4318/api/health", {
      headers: { authorization: `Bearer ${env.FILEM_DEV_TOKEN}` },
    });
    if (res.ok) {
      ready = true;
      break;
    }
  } catch {}
  await new Promise((resolve) => setTimeout(resolve, 1000));
}
if (ready)
  start(process.execPath, [
    resolve(import.meta.dirname, "../node_modules/vite/bin/vite.js"),
  ]);
else {
  console.error("Rust 服务未能启动，请检查上方输出。");
  stop(1);
}
