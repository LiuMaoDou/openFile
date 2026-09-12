import {
  CheckCircle2,
  Clock3,
  LoaderCircle,
  AlertCircle,
  PauseCircle,
} from "lucide-react";
import type { Scope } from "../types";
import { formatDate } from "./FileIcon";
const pending = (scope: Scope) =>
  ["scanning", "verifying", "dirty", "unscanned"].includes(scope.freshness);
export function scanLabel(scope: Scope, paused: boolean) {
  if (scope.availability !== "available")
    return scope.availability === "offline" ? "文件夹离线" : "无法访问";
  if (paused && pending(scope)) return "扫描已暂停";
  if (pending(scope))
    return scope.freshness === "scanning" ? "扫描中" : "等待核对";
  return scope.freshness === "current" ? "扫描完成" : "扫描未完成";
}
export function ScanStatus({
  scopes,
  paused,
}: {
  scopes: Scope[];
  paused: boolean;
}) {
  if (!scopes.length) return null;
  const issues = scopes.filter(
    (s) => s.availability !== "available" || s.freshness === "partial",
  );
  const running = scopes.filter(pending);
  const last = Math.max(...scopes.map((s) => s.lastScan ?? 0));
  const Icon =
    paused && running.length
      ? PauseCircle
      : running.length
        ? LoaderCircle
        : issues.length
          ? AlertCircle
          : CheckCircle2;
  return (
    <div
      className={`scan-status ${issues.length && !running.length ? "incomplete" : ""}`}
      role="status"
      title={issues
        .map((s) => `${s.name}：${s.message || scanLabel(s, paused)}`)
        .join("\n")}
    >
      <Icon size={16} className={!paused && running.length ? "spin" : ""} />
      <strong>
        {paused && running.length
          ? "扫描已暂停"
          : running.length
            ? `正在扫描 ${running.length} 个文件夹`
            : issues.length
              ? `${issues.length} 个文件夹需要核对`
              : "扫描完成"}
      </strong>
      <span>
        {running.length
          ? `已发现 ${running.reduce((n, s) => n + s.scanned, 0).toLocaleString()} 个文件${paused ? " · 文件操作完成后继续" : ""}`
          : `${scopes.length} 个文件夹已检查`}
      </span>
      {!running.length && last > 0 && (
        <span className="scan-completed">
          <Clock3 size={13} />
          {issues.length ? "最近扫描" : "完成于"} {formatDate(last)}
        </span>
      )}
    </div>
  );
}
