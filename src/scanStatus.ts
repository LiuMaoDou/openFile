import type { Scope, ScanRun } from "./types";

export const scanPending = (scope: Scope) =>
  scope.availability === "available" &&
  ["scanning", "verifying", "dirty", "unscanned"].includes(scope.freshness);

// A run owns one immutable directory plan. Never add another run (a rescan or
// newly added folder) to its denominator, including completed historical runs.
export function scanStatus(scopes: Scope[], runs: ScanRun[], paused: boolean) {
  const ids = new Set(scopes.map((scope) => scope.id));
  const visible = runs.filter((run) => run.scopeIds.some((id) => ids.has(id)));
  const active = visible.filter((run) => !run.finished);
  const run =
    active[0] ??
    visible.reduce<ScanRun | undefined>(
      (last, candidate) =>
        !last || candidate.round > last.round ? candidate : last,
      undefined,
    );
  const waiting = scopes.filter(
    (scope) =>
      scanPending(scope) &&
      !active.some((task) => task.scopeIds.includes(scope.id)),
  );
  const issues = scopes.filter(
    (scope) =>
      scope.availability !== "available" || scope.freshness === "partial",
  );
  const busy = active.length > 0 || waiting.length > 0;
  const counting =
    !!active.length && active.some((task) => task.totalDirectories === null);
  const finalizing =
    !!active.length &&
    active.every(
      (task) =>
        task.phase === "finalizing" ||
        (task.totalDirectories !== null &&
          task.processedDirectories >= task.totalDirectories),
    );
  const stalled = !paused && active.some((task) => task.idleMs >= 60000);
  // Multiple runs are not one batch. This also handles older backend snapshots
  // without inventing a global percentage from unrelated plans.
  const percent =
    active.length === 1 &&
    !counting &&
    !finalizing &&
    run &&
    run.totalDirectories !== null &&
    run.totalDirectories > 0
      ? Math.floor((run.processedDirectories / run.totalDirectories) * 100)
      : null;
  const title =
    paused && busy
      ? "扫描已暂停"
      : active.length
        ? stalled
          ? "扫描暂无进展"
          : counting
            ? "正在统计目录"
            : finalizing
              ? "正在整理索引"
              : "正在扫描文件"
        : waiting.length
          ? "等待扫描"
          : run?.phase === "failed"
            ? "扫描失败"
            : run?.phase === "cancelled"
              ? "扫描已取消"
              : issues.length || run?.phase === "partial"
                ? "扫描已结束，部分文件夹未完成"
                : "扫描完成";
  return {
    active,
    run,
    waiting,
    issues,
    busy,
    counting,
    finalizing,
    stalled,
    percent,
    title,
    paused: paused && busy,
    warning:
      stalled ||
      (!busy &&
        (issues.length > 0 ||
          run?.phase === "failed" ||
          run?.phase === "partial")),
  };
}
