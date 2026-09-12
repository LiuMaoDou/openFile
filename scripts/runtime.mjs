import { existsSync } from "node:fs";
import { delimiter, resolve } from "node:path";
export function rustEnvironment() {
  const root = resolve(import.meta.dirname, "..");
  const cargoHome = resolve(root, ".tools/cargo");
  const local = existsSync(
    resolve(
      cargoHome,
      "bin",
      process.platform === "win32" ? "cargo.exe" : "cargo",
    ),
  );
  return {
    ...process.env,
    ...(local
      ? {
          CARGO_HOME: cargoHome,
          RUSTUP_HOME: resolve(root, ".tools/rustup"),
          PATH: `${resolve(cargoHome, "bin")}${delimiter}${process.env.PATH}`,
        }
      : {}),
  };
}
