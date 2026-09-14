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
function elapsedLabel(ms: number) {
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分 ${seconds % 60} 秒`;
  return `${Math.floor(minutes / 60)} 小时 ${minutes % 60} 分`;
}
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
      {running.some((scope) => scope.progress) && (
        <div className="scan-progress-list" aria-live="off">
          {running.map((scope) => {
            const progress = scope.progress;
            if (!progress) return null;
            const total = Math.max(1, progress.discoveredDirectories);
            const processed = Math.min(total, progress.processedDirectories);
            const percent = Math.floor((processed / total) * 100);
            const phase = paused
              ? "扫描已暂停"
              : progress.phase === "finalizing"
                ? "目录已扫描完，正在整理索引"
                : progress.phase === "indexing"
                  ? "正在写入索引"
                  : "正在扫描目录";
            return (
              <div className="scan-progress-item" key={scope.id}>
                <div className="scan-progress-heading">
                  <strong title={scope.path}>{scope.name}</strong>
                  <span>{phase}</span>
                  <span className="scan-progress-time">
                    已用时 {elapsedLabel(progress.elapsedMs)}
                  </span>
                </div>
                <div className="scan-progress-meter">
                  <progress
                    max={total}
                    value={processed}
                    aria-label={`${scope.name} 已发现目录处理进度`}
                    aria-valuetext={`已处理 ${processed} 个目录，已发现 ${total} 个目录，${percent}%`}
                  />
                  <strong>{percent}%</strong>
                </div>
                <div className="scan-progress-detail">
                  <span>
                    已处理目录 {processed.toLocaleString()} /{" "}
                    {total.toLocaleString()}
                  </span>
                  <span>总目录数随扫描更新</span>
                </div>
                <div
                  className="scan-progress-path"
                  title={progress.currentPath}
                >
                  当前位置：{progress.currentPath}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
