export const isDesktop = () => "__TAURI_INTERNALS__" in window;

export async function command<T>(
  name: string,
  args: unknown = {},
  signal?: AbortSignal,
): Promise<T> {
  if (isDesktop()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("command", { command: name, args });
  }
  const response = await fetch("/api/command", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ command: name, args }),
    signal,
  });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || "无法连接本地索引服务。");
  return result as T;
}
export async function pickDirectory(): Promise<string | null> {
  if (!isDesktop()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({
    directory: true,
    multiple: false,
    title: "选择文件夹",
  });
  return typeof result === "string" ? result : null;
}
export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
