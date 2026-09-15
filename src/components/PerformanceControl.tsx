import { useEffect, useState } from "react";
import { command, errorText } from "../api";
interface Performance {
  mode: "auto" | "low" | "balanced" | "fast";
  scanThreads: number;
  contentThreads: number;
}
export function PerformanceControl() {
  const [setting, setSetting] = useState<Performance | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    command<Performance>("performance", {}, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) setSetting(value);
      })
      .catch((error) => {
        if (!controller.signal.aborted) setError(errorText(error));
      });
    return () => controller.abort();
  }, []);
  async function change(mode: string) {
    setBusy(true);
    setError("");
    try {
      setSetting(await command<Performance>("set_performance", { mode }));
    } catch (error) {
      setError(errorText(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="scan-performance">
      <label>
        扫描性能
        <select
          aria-label="扫描性能"
          disabled={!setting || busy}
          value={setting?.mode ?? "auto"}
          onChange={(event) => void change(event.target.value)}
        >
          <option value="auto">自动</option>
          <option value="low">低占用</option>
          <option value="balanced">均衡</option>
          <option value="fast">高性能</option>
        </select>
      </label>
      {setting && (
        <span>
          {setting.scanThreads} 个扫描线程 · {setting.contentThreads}{" "}
          个正文解析线程；扫描设置在下一批生效。
        </span>
      )}
      {error && <span role="alert">{error}</span>}
    </div>
  );
}
