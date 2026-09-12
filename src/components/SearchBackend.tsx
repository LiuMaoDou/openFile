import { useEffect, useState } from "react";
import { Zap, RefreshCw } from "lucide-react";
import { command, errorText } from "../api";
import type { QueryResult } from "../types";
interface Backend {
  supported: boolean;
  available: boolean;
  enabled: boolean;
  message: string;
}
export function SearchBackend({
  changed,
  result,
}: {
  changed: () => void;
  result: QueryResult;
}) {
  const [status, setStatus] = useState<Backend | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let stopped = false;
    command<Backend>("everything_status")
      .then((value) => {
        if (!stopped) setStatus(value);
      })
      .catch((e) => {
        if (!stopped) setError(errorText(e));
      });
    return () => {
      stopped = true;
    };
  }, []);
  async function update(enabled?: boolean) {
    setBusy(true);
    setError("");
    try {
      setStatus(
        await command<Backend>(
          enabled === undefined ? "everything_status" : "set_everything",
          { enabled },
        ),
      );
      if (enabled !== undefined) changed();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="search-backend">
      <label title={status?.message}>
        <Zap size={14} />
        <input
          type="checkbox"
          aria-label="Everything 加速"
          disabled={busy || !status?.supported}
          checked={status?.enabled ?? false}
          onChange={(e) => void update(e.target.checked)}
        />
        Everything 加速
      </label>
      <span>
        {error ||
          result.searchNotice ||
          (status?.enabled
            ? result.searchEngine === "everything"
              ? "本次搜索：Everything SDK"
              : status.message
            : status?.supported
              ? "使用本地索引 · 可连接 Everything SDK"
              : "本地索引 · Everything 仅支持 Windows")}
      </span>
      {status?.supported && (
        <button
          className="icon-button"
          aria-label="检查 Everything 连接"
          title="检查 Everything 连接"
          disabled={busy}
          onClick={() => void update()}
        >
          <RefreshCw size={13} className={busy ? "spin" : ""} />
        </button>
      )}
    </div>
  );
}
