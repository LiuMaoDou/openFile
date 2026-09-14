import {
  CheckCircle2,
  Clock3,
  LoaderCircle,
  AlertCircle,
  PauseCircle,
} from "lucide-react";
import type { Scope, ScanRun } from "../types";
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
  if (pending(scope) && scope.progress?.phase === "counting")
    return "统计目录中";
  if (pending(scope))
    return scope.freshness === "scanning" ? "扫描中" : "等待核对";
  return scope.freshness === "current" ? "扫描完成" : "扫描未完成";
}
const phases: Record<ScanRun["phase"], string> = {
  counting: "正在统计目录",
  scanning: "正在扫描文件",
  indexing: "正在写入索引",
  finalizing: "正在整理索引",
  complete: "扫描完成",
  partial: "部分目录需要核对",
  cancelled: "扫描已取消",
  failed: "扫描中断",
};
export function ScanStatus({
  scopes,
  runs = [],
  paused,
}: {
  scopes: Scope[];
  runs: ScanRun[];
  paused: boolean;
}) {
  if (!scopes.length) return null;
  const issues = scopes.filter(
    (s) => s.availability !== "available" || s.freshness === "partial",
  );
  const running = scopes.filter(pending);
  const visible = runs.filter((r) =>
    r.scopeIds.some((id) => scopes.some((s) => s.id === id)),
  );
  const active = visible.filter((r) => !r.finished);
  const counting = active.some((r) => r.totalDirectories === null);
  const waiting = running.filter(
    (s) => !active.some((r) => r.scopeIds.includes(s.id)),
  );
  const total = visible.reduce(
    (n, r) => n + (r.totalDirectories ?? r.discoveredDirectories),
    0,
  );
  const processed = visible.reduce((n, r) => n + r.processedDirectories, 0);
  const checked = visible.reduce((n, r) => n + r.checkedFiles, 0);
  const complete =
    visible.length > 0 &&
    visible.every((r) => r.phase === "complete") &&
    !running.length;
  const determinate =
    visible.length > 0 &&
    !counting &&
    !waiting.length &&
    visible.every((r) => r.totalDirectories !== null);
  const percent = complete
    ? 100
    : Math.min(99, Math.floor((processed / Math.max(1, total)) * 100));
  const stalled = !paused && active.some((r) => r.idleMs >= 60000);
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
      className={`scan-status ${stalled || (issues.length && !running.length) ? "incomplete" : ""}`}
      role="status"
    >
      <Icon size={16} className={!paused && running.length ? "spin" : ""} />
      <strong>
        {paused && running.length
          ? "扫描已暂停"
          : counting
            ? "全局进度 · 正在统计目录"
            : active.length
              ? "全局扫描进度"
              : running.length
                ? "等待核对文件变化"
                : issues.length
                  ? "部分文件夹需要核对"
                  : "扫描完成"}
      </strong>
      <span>
        {visible.length
          ? `本轮已核对 ${checked.toLocaleString()} 个文件`
          : `${scopes.length} 个监控文件夹`}
      </span>
      {!running.length && last > 0 && (
        <span className="scan-completed">
          <Clock3 size={13} />
          最近完成于 {formatDate(last)}
        </span>
      )}
      {visible.length > 0 && (
        <div className="scan-progress-list" aria-live="off">
          <div className="scan-progress-item">
            <div className="scan-progress-meter">
              <progress
                max={100}
                value={determinate ? percent : active.length ? undefined : 0}
                aria-label="全局目录扫描进度"
                aria-valuetext={
                  determinate
                    ? `已处理 ${processed} / ${total} 个目录，${percent}%`
                    : `${active.length ? "正在统计目录" : "统计未完成"}，已发现 ${total} 个`
                }
              />
              <strong>
                {determinate
                  ? `${percent}%`
                  : active.length
                    ? "统计中"
                    : waiting.length
                      ? "待核对"
                      : "已停止"}
              </strong>
            </div>
            <div className="scan-progress-detail">
              <span>
                {counting
                  ? `已发现 ${total.toLocaleString()} 个目录，统计完成后显示百分比`
                  : `已处理目录 ${processed.toLocaleString()} / ${total.toLocaleString()}`}
              </span>
              {waiting.length > 0 && (
                <span>{waiting.length} 个文件夹等待核对</span>
              )}
              {!counting && (
                <span>
                  按本轮目录清单计算 · 目录大小不同，百分比不代表剩余时间
                </span>
              )}
            </div>
          </div>
          {visible.map((run) => (
            <div className="scan-progress-item" key={run.round}>
              <div className="scan-progress-heading">
                <strong>
                  第 {run.round} 轮 ·{" "}
                  {paused && !run.finished ? "扫描已暂停" : phases[run.phase]}
                </strong>
                <span>
                  {run.scopeIds
                    .map((id) => scopes.find((s) => s.id === id)?.name)
                    .filter(Boolean)
                    .join("、")}
                </span>
                <span className="scan-progress-time">
                  已用时 {elapsedLabel(run.elapsedMs)}
                </span>
              </div>
              <div className="scan-progress-detail">
                <span>原因：{run.reason}</span>
                {run.phase === "counting" && (
                  <span>
                    已枚举 {run.visitedEntries.toLocaleString()} 个条目
                  </span>
                )}
                {!run.finished && (
                  <span>
                    {paused
                      ? "文件操作结束后继续"
                      : `距上次推进 ${elapsedLabel(run.idleMs)}`}
                  </span>
                )}
              </div>
              {!run.finished && (
                <div className="scan-progress-path" title={run.currentPath}>
                  当前位置：{run.currentPath || "正在准备"}
                </div>
              )}
              {!run.finished && !paused && run.idleMs >= 60000 && (
                <p className="scan-stalled">
                  已 {elapsedLabel(run.idleMs)}{" "}
                  没有推进，可能正在等待磁盘或路径响应。可取消扫描，并在文件夹设置中关闭“监听文件变化”后核对一次；轮次持续增加说明发生了重新扫描。
                </p>
              )}
            </div>
          ))}
          {visible.some((r) => r.round > 1) && (
            <div className="scan-progress-detail">
              轮次按本次软件启动累计；新加入的扫描任务或重新核对都会增加轮次。
            </div>
          )}
        </div>
      )}
    </div>
  );
}
