import { File } from "lucide-react";
import extensions from "../assets/file-icons/extensions.json";
const icons = import.meta.glob<string>("../assets/file-icons/*.svg", {
  query: "?url&no-inline",
  import: "default",
  eager: true,
});
const suffixes: Record<string, string> = extensions;

export function FileIcon({
  extension,
  group,
  name,
  large = false,
}: {
  extension: string;
  group: string;
  name?: string;
  large?: boolean;
}) {
  const parts = name?.toLowerCase().split(".") ?? [];
  let icon = suffixes[extension.toLowerCase()];
  for (let i = 1; i < parts.length; i++) {
    const match = suffixes[parts.slice(i).join(".")];
    if (match) {
      icon = match;
      break;
    }
  }
  const src = icon && icons[`../assets/file-icons/${icon}.svg`];
  return (
    <span
      className={`file-icon ${large ? "large" : ""} ${src ? "branded" : "fallback"}`}
      data-group={group}
      title={extension.toUpperCase() || "无扩展名"}
    >
      {src ? (
        <img
          src={src}
          alt=""
          width={large ? 76 : 28}
          height={large ? 76 : 28}
          draggable={false}
        />
      ) : (
        <File aria-hidden size={large ? 76 : 28} strokeWidth={1.5} />
      )}
      {(large || !src) && <span>{extension.toUpperCase() || "FILE"}</span>}
    </span>
  );
}
export function formatSize(value: number) {
  if (value < 1024) return `${value} B`;
  const i = Math.min(Math.floor(Math.log(value) / Math.log(1024)), 4);
  return `${(value / 1024 ** i).toFixed(1)} ${["B", "KB", "MB", "GB", "TB"][i]}`;
}
export function formatDate(value: number) {
  return value
    ? new Intl.DateTimeFormat("zh-CN", {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        hour12: false,
      }).format(value)
    : "—";
}
