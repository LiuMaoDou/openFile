import { useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  LoaderCircle,
  AlertCircle,
  PauseCircle,
  Clock3,
} from "lucide-react";
import type { Scope, ScanRun } from "../types";
import { scanPending, scanStatus } from "../scanStatus";
import { formatDate } from "./FileIcon";
import { ScanIssues } from "./ScanIssues";
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
  if (paused && scanPending(scope)) return "扫描已暂停";
  if (scanPending(scope) && scope.progress?.phase === "counting")
    return "统计目录中";
  if (scanPending(scope))
    return scope.freshness === "scanning" ? "扫描中" : "等待核对";
  return scope.freshness === "current" ? "扫描完成" : "扫描未完成";
}
const phases: Record<ScanRun["phase"], string> = {
  counting: "正在统计目录",
  scanning: "正在扫描文件",
  indexing: "正在写入索引",
  finalizing: "正在整理索引",
  complete: "扫描完成",
  partial: "扫描结束，有未完成项",
  cancelled: "扫描已取消",
  failed: "扫描失败",
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
  const [expanded, setExpanded] = useState(false);
  if (!scopes.length) return null;
  const state = scanStatus(scopes, runs, paused);
  const { run } = state;
  const Icon = state.paused
    ? PauseCircle
    : state.warning
      ? AlertCircle
      : state.active.length
        ? LoaderCircle
        : state.busy
          ? Clock3
          : run?.phase === "cancelled"
            ? PauseCircle
            : CheckCircle2;
  const metric =
    state.active.length > 1
      ? `${state.active.length} 个扫描任务`
      : state.counting
        ? `已发现 ${(run?.discoveredDirectories ?? 0).toLocaleString()} 个目录`
        : state.percent !== null
          ? `${run!.processedDirectories.toLocaleString()} / ${run!.totalDirectories!.toLocaleString()} 个目录`
          : state.finalizing
            ? "目录已处理，正在收尾"
            : state.waiting.length
              ? `${state.waiting.length} 个文件夹待扫描`
              : state.issues.length
                ? `${state.issues.length} 个文件夹需处理`
                : `${scopes.length} 个文件夹`;
  return (
    <details
      className={`scan-status${state.warning ? " incomplete" : ""}${state.paused ? " paused" : ""}`}
      onToggle={(event) => setExpanded(event.currentTarget.open)}
    >
      <summary
        className={`scan-status-summary${state.active.length > 0 && !state.finalizing ? " has-progress" : ""}`}
      >
        <span className="scan-status-label" role="status" title={state.title}>
          <Icon
            size={15}
            className={
              !state.paused && !state.stalled && state.active.length
                ? "spin"
                : ""
            }
          />
          <strong>{state.title}</strong>
        </span>
        {state.active.length > 0 && !state.finalizing && (
          <span className="scan-progress-meter">
            <progress
              max={100}
              value={state.percent ?? undefined}
              aria-label="本批次目录扫描进度"
              aria-valuetext={
                state.percent !== null
                  ? `${metric}，${state.percent}%`
                  : state.title
              }
            />
            {state.percent !== null && <strong>{state.percent}%</strong>}
          </span>
        )}
        <span className="scan-status-metric" title={metric}>
          {metric}
        </span>
        <span className="scan-status-toggle">
          详情 <ChevronDown size={14} />
        </span>
      </summary>
      <div className="scan-status-details">
        {state.issues.map((scope) => (
          <div key={scope.id} className="scan-issue">
            <p>
              <strong>{scope.name}</strong>：
              {scope.message ||
                (scope.availability === "offline"
                  ? "文件夹离线，请检查磁盘连接。"
                  : scope.availability !== "available"
                    ? "无法访问，请检查路径和权限。"
                    : "扫描结果不完整，可在文件夹设置中检查后重新扫描。")}
            </p>
            {expanded && scope.scanIssueCount > 0 && (
              <ScanIssues scope={scope} />
            )}
            {!scope.scanIssueCount &&
              scope.message?.includes("个路径无法完整扫描") && (
                <p className="field-hint">
                  这条记录尚未保存具体路径。点击“刷新”重新扫描后，会在这里列出路径和失败原因。
                </p>
              )}
          </div>
        ))}
        {run && (
          <p>
            本次范围：
            {run.scopeIds
              .map((id) => scopes.find((scope) => scope.id === id)?.name)
              .filter(Boolean)
              .join("、")}
          </p>
        )}
        {state.busy && (
          <p>
            先统计本批次的目录总数，再按已处理目录计算进度。百分比表示目录处理比例；目录大小不同，不代表剩余时间。
          </p>
        )}
        {state.active.length > 0 && state.waiting.length > 0 && (
          <p>{state.waiting.length} 个文件夹等待下一批扫描，不计入当前进度。</p>
        )}
        {(state.active.length ? state.active : run ? [run] : []).map((task) => (
          <div className="scan-task-detail" key={task.round}>
            <p>
              {phases[task.phase]} · 已核对 {task.checkedFiles.toLocaleString()}{" "}
              个文件 · 用时 {elapsedLabel(task.elapsedMs)}
            </p>
            <p>
              任务 {task.round} · {task.reason}
            </p>
            {task.totalDirectories !== null && (
              <p>
                已处理 {task.processedDirectories.toLocaleString()} /{" "}
                {task.totalDirectories.toLocaleString()} 个目录
              </p>
            )}
            {!task.finished && (
              <p className="scan-progress-path">
                当前位置：{task.currentPath || "正在准备"}
              </p>
            )}
            {!task.finished && !paused && task.idleMs >= 60000 && (
              <p className="scan-stalled">
                已 {elapsedLabel(task.idleMs)}{" "}
                没有推进，可能正在等待磁盘或路径响应。可取消扫描后检查上述路径。
              </p>
            )}
          </div>
        ))}
        {!state.busy &&
          !state.issues.length &&
          scopes.some((scope) => scope.lastScan) && (
            <p>
              最近核对：
              {formatDate(
                Math.max(...scopes.map((scope) => scope.lastScan ?? 0)),
              )}
            </p>
          )}
      </div>
    </details>
  );
}
