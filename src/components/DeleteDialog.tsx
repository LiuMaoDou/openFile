import { useEffect, useRef, useState } from "react";
import { FolderInput, Trash2, X } from "lucide-react";
import { command, errorText } from "../api";
import type { DeleteItem, DeletePlan } from "../types";
import { formatSize } from "./FileIcon";

const labels: Record<DeleteItem["status"], string> = {
  ready: "等待处理",
  blocked: "不可删除",
  running: "处理中",
  succeeded: "已移入回收站",
  failed: "未删除",
  unknown: "需要核对",
  cancelled: "未执行",
};

export function DeleteDialog({
  ids,
  close,
  completed,
  action = "delete",
  destination,
}: {
  ids?: string[];
  action?: "delete" | "move";
  destination?: string;
  close: () => void;
  completed: (ids: string[]) => void;
}) {
  const moving = action === "move";
  const verb = moving ? "移动" : "删除";
  const dialog = useRef<HTMLDialogElement>(null);
  const submitted = useRef(false);
  const alive = useRef(true);
  const completedRef = useRef(completed);
  completedRef.current = completed;
  const [plan, setPlan] = useState<DeletePlan | null>(null);
  const [history, setHistory] = useState<DeletePlan[]>([]);
  const [loading, setLoading] = useState(true);
  const [executing, setExecuting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState("");
  const [expired, setExpired] = useState(false);
  const busy = executing || plan?.state === "running";
  const ready =
    plan?.items.filter((item) => item.status === "ready").length ?? 0;
  const succeeded =
    plan?.items.filter((item) => item.status === "succeeded").length ?? 0;
  const failed =
    plan?.items.filter((item) =>
      ["failed", "unknown", "blocked"].includes(item.status),
    ).length ?? 0;

  useEffect(() => {
    alive.current = true;
    dialog.current?.showModal();
    let ignore = false;
    async function load() {
      try {
        if (ids) {
          const value = await command<DeletePlan>(`preview_${action}`, {
            ids,
            destination,
          });
          if (!ignore) setPlan(value);
        } else {
          const values = await command<DeletePlan[]>(`${action}_history`);
          if (!ignore) {
            setHistory(values);
            setPlan(values[0] ?? null);
          }
        }
      } catch (error) {
        if (!ignore) setError(errorText(error));
      } finally {
        if (!ignore) setLoading(false);
      }
    }
    void load();
    return () => {
      ignore = true;
      alive.current = false;
    };
  }, [ids, action, destination]);

  useEffect(() => {
    if (!plan || plan.state !== "planned") return;
    const timer = setTimeout(
      () => setExpired(true),
      Math.max(0, plan.expires - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [plan?.id, plan?.state, plan?.expires]);

  useEffect(() => {
    if (!plan || plan.state !== "running") return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const id = plan.id;
    async function poll() {
      try {
        const next = await command<DeletePlan>(`${action}_plan`, { id });
        if (cancelled) return;
        if (next.state === "planned") {
          timer = setTimeout(poll, 800);
          return;
        }
        setPlan((current) =>
          current?.id === next.id && current.state === "running"
            ? next
            : current,
        );
        if (next.state !== "running") {
          completedRef.current(
            next.items
              .filter((item) => item.status === "succeeded")
              .map((item) => item.id),
          );
          return;
        }
      } catch (error) {
        if (!cancelled) setError(errorText(error));
      }
      if (!cancelled) timer = setTimeout(poll, 800);
    }
    timer = setTimeout(poll, 800);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [plan?.id, plan?.state]);

  async function execute() {
    if (!plan || submitted.current || !ready || expired) return;
    submitted.current = true;
    setExecuting(true);
    setError("");
    setPlan({ ...plan, state: "running" });
    try {
      const result = await command<DeletePlan>(`execute_${action}`, {
        id: plan.id,
      });
      if (alive.current) setPlan(result);
      completedRef.current(
        result.items
          .filter((item) => item.status === "succeeded")
          .map((item) => item.id),
      );
    } catch (error) {
      if (alive.current) {
        setError(
          `${errorText(error)} 请查看逐项记录并核对源文件；本窗口不会重复提交。`,
        );
        try {
          const next = await command<DeletePlan>(`${action}_plan`, {
            id: plan.id,
          });
          if (alive.current) setPlan(next);
        } catch {
          /* Keep the last known result when disconnected. */
        }
      }
    } finally {
      if (alive.current) setExecuting(false);
    }
  }
  async function stop() {
    if (!plan) return;
    setStopping(true);
    try {
      await command(`cancel_${action}`, { id: plan.id });
    } catch (error) {
      setError(errorText(error));
      setStopping(false);
    }
  }
  return (
    <dialog
      ref={dialog}
      className="dialog delete-dialog"
      aria-labelledby="delete-title"
      onCancel={close}
    >
      <header className="dialog-header">
        <div>
          <h2 id="delete-title">
            {!ids
              ? `${verb}记录`
              : (plan?.state === "planned" && !submitted.current) || loading
                ? `确认${verb}文件`
                : busy
                  ? moving
                    ? "正在移动文件"
                    : "正在移入回收站"
                  : `${verb}结果`}
          </h2>
          <p>
            {ids && plan?.state === "planned"
              ? moving
                ? "文件将移动到目标文件夹，同名文件会跳过。"
                : "这些真实文件将离开原目录，移入系统回收站。"
              : moving
                ? "按文件记录移动结果。"
                : "按文件记录结果，可到系统回收站查看和恢复。"}
          </p>
        </div>
        <button
          className="icon-button"
          aria-label={`关闭${verb}窗口`}
          onClick={close}
        >
          <X size={20} />
        </button>
      </header>
      <div className="dialog-body delete-body">
        {!ids && history.length > 0 && (
          <label className="field">
            最近 10 批操作
            <select
              className="history-select"
              aria-label={`选择${verb}记录`}
              disabled={busy}
              value={plan?.id ?? ""}
              onChange={(event) => {
                setPlan(
                  history.find((entry) => entry.id === event.target.value) ??
                    null,
                );
                setStopping(false);
                setError("");
              }}
            >
              {history.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {new Date(entry.created).toLocaleString("zh-CN", {
                    hour12: false,
                  })}{" "}
                  · {entry.items.length} 个文件
                </option>
              ))}
            </select>
          </label>
        )}
        {loading ? (
          <p className="delete-empty" role="status">
            正在检查文件…
          </p>
        ) : !plan && !error ? (
          <p className="delete-empty">暂无{verb}记录</p>
        ) : null}
        {plan && (
          <>
            {moving && (
              <p className="move-destination">
                目标文件夹：<strong>{plan.destination || destination}</strong>
              </p>
            )}
            <div className="delete-summary" role="status">
              {plan.state === "planned" ? (
                <>
                  <strong>
                    {ready} 个可{verb}
                  </strong>
                  <span>
                    {formatSize(
                      plan.items
                        .filter((item) => item.status === "ready")
                        .reduce((total, item) => total + item.size, 0),
                    )}
                  </span>
                  {failed > 0 && <span>{failed} 个将跳过</span>}
                </>
              ) : (
                <>
                  <strong>
                    {succeeded} 个{moving ? "已移动" : "已移入回收站"}
                  </strong>
                  <span>{failed} 个需处理</span>
                  <span>
                    {
                      plan.items.filter((item) =>
                        ["ready", "running", "cancelled"].includes(item.status),
                      ).length
                    }{" "}
                    个{busy ? "待完成" : "未完成"}
                  </span>
                </>
              )}
            </div>
            <ul className="delete-list" aria-label={`待${verb}文件及处理结果`}>
              {plan.items.map((item) => (
                <li key={item.id}>
                  <div className="delete-item-heading">
                    <strong>{item.name}</strong>
                    <span className={`delete-status ${item.status}`}>
                      {item.status === "ready"
                        ? plan.state === "planned" && !submitted.current
                          ? moving
                            ? "待移动"
                            : "待移入回收站"
                          : busy
                            ? "等待处理"
                            : "未执行"
                        : moving && item.status === "succeeded"
                          ? "已移动"
                          : moving && item.status === "failed"
                            ? "未移动"
                            : moving && item.status === "blocked"
                              ? plan.state === "planned"
                                ? "将跳过"
                                : "已跳过"
                              : labels[item.status]}
                    </span>
                  </div>
                  <p className="delete-path">{item.path || "原路径已失效"}</p>
                  {item.message && (
                    <p className="delete-message">{item.message}</p>
                  )}
                </li>
              ))}
            </ul>
            {!moving && (
              <p className="field-hint">
                删除前再次核对文件。回收站不可用时会报错，不会转为永久删除。macOS
                可从废纸篓拖回文件，部分系统不提供“放回原处”。
              </p>
            )}
            {moving && (
              <p className="field-hint">
                目标不在监控文件夹内时，文件会从列表消失；可添加目标文件夹继续浏览。
              </p>
            )}
            {plan.state === "interrupted" && (
              <p className="inline-info">
                上次操作中断。标记“需要核对”的文件请检查
                {moving ? "原位置和目标位置" : "源路径与回收站"}
                ，未执行的项目不会自动重试。
              </p>
            )}
            {expired && plan.state === "planned" && (
              <p className="inline-error">
                这份预览已超过 10 分钟，请关闭后重新选择。
              </p>
            )}
          </>
        )}
        {error && (
          <div className="inline-error" role="alert">
            {error}
          </div>
        )}
      </div>
      <footer className="dialog-footer">
        <span className="spacer" />
        {busy && (
          <button className="button" onClick={close}>
            后台处理
          </button>
        )}
        {busy ? (
          <button className="button outline" disabled={stopping} onClick={stop}>
            {stopping ? `正在停止后续${verb}…` : `停止后续${verb}`}
          </button>
        ) : (
          <button autoFocus className="button outline" onClick={close}>
            {plan?.state === "planned" && !submitted.current ? "取消" : "关闭"}
          </button>
        )}
        {ids && plan?.state === "planned" && !submitted.current && (
          <button
            className={`button ${moving ? "primary" : "danger"}`}
            disabled={!ready || expired || submitted.current}
            onClick={execute}
          >
            {moving ? <FolderInput size={16} /> : <Trash2 size={16} />}
            {moving ? "确认移动" : "移入回收站"}（{ready}）
          </button>
        )}
      </footer>
    </dialog>
  );
}
