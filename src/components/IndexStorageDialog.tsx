import { useEffect, useRef, useState } from "react";
import { FolderOpen, RefreshCw, X } from "lucide-react";
import { command, errorText, pickDirectory } from "../api";

interface StorageInfo {
  path: string;
  databaseBytes: number;
  walBytes: number;
  totalBytes: number;
  pendingPath: string | null;
  previousPath: string | null;
  migrationError: string | null;
  canChange: boolean;
}

function bytes(value: number) {
  if (value < 1024) return `${value} B`;
  const unit = value >= 1024 ** 3 ? 3 : value >= 1024 ** 2 ? 2 : 1;
  return `${(value / 1024 ** unit).toFixed(2)} ${["B", "KiB", "MiB", "GiB"][unit]}`;
}

export function IndexStorageDialog({ close }: { close: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [info, setInfo] = useState<StorageInfo | null>(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  useEffect(() => {
    dialog.current?.showModal();
    const controller = new AbortController();
    command<StorageInfo>("index_storage", {}, controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) setInfo(data);
      })
      .catch((error) => {
        if (!controller.signal.aborted) setError(errorText(error));
      });
    return () => controller.abort();
  }, []);

  async function run(action: string, args = {}, success = "") {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      const result = await command<StorageInfo>(action, args);
      if (action !== "reveal_index") setInfo(result);
      if (action === "set_index_location") setPath("");
      setMessage(success);
    } catch (error) {
      setError(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  async function browse() {
    setBusy(true);
    setError("");
    try {
      const selected = await pickDirectory();
      if (selected) setPath(selected);
    } catch (error) {
      setError(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialog}
      className="dialog index-storage-dialog"
      aria-labelledby="index-storage-title"
      onCancel={(event) => {
        if (busy) event.preventDefault();
        else close();
      }}
    >
      <header className="dialog-header">
        <div>
          <h2 id="index-storage-title">索引存储</h2>
          <p>查看本机索引占用，选择后续扫描使用的存放位置。</p>
        </div>
        <button
          className="icon-button"
          aria-label="关闭索引存储"
          disabled={busy}
          onClick={close}
        >
          <X size={20} />
        </button>
      </header>
      <div className="dialog-body index-storage-body" aria-busy={busy}>
        {error && (
          <div className="inline-error" role="alert">
            {error}
          </div>
        )}
        {!info && !error && <p role="status">正在读取索引位置…</p>}
        {!info && error && (
          <button
            className="button outline"
            disabled={busy}
            onClick={() => run("index_storage")}
          >
            重试
          </button>
        )}
        {info && (
          <>
            <h3>当前索引文件</h3>
            <p className="index-storage-path">{info.path}</p>
            <div className="index-storage-actions">
              <button
                className="button outline"
                disabled={busy}
                onClick={() => run("reveal_index")}
              >
                <FolderOpen size={16} />
                定位索引文件
              </button>
              <button
                className="text-button"
                disabled={busy}
                onClick={() => run("index_storage")}
              >
                <RefreshCw size={14} />
                刷新占用
              </button>
            </div>
            <dl className="index-storage-sizes">
              <div>
                <dt>索引数据库</dt>
                <dd>{bytes(info.databaseBytes)}</dd>
              </div>
              <div>
                <dt>写入日志（WAL）</dt>
                <dd>{bytes(info.walBytes)}</dd>
              </div>
              <div>
                <dt>索引文件合计</dt>
                <dd>{bytes(info.totalBytes)}</dd>
              </div>
            </dl>
            {info.migrationError && (
              <p className="inline-error" role="alert">
                {info.migrationError}
              </p>
            )}
            {info.pendingPath && (
              <div className="index-storage-pending">
                <h3>已保存 · 重启后生效</h3>
                <p className="index-storage-path">{info.pendingPath}</p>
                <p>
                  请退出并重新打开
                  FileM。启动时先迁移已有索引，再从新位置扫描；本次运行仍使用上面的当前位置。
                </p>
                <button
                  className="text-button"
                  disabled={busy}
                  onClick={() =>
                    run(
                      "cancel_index_location",
                      {},
                      "已取消切换，继续使用当前位置。",
                    )
                  }
                >
                  取消切换
                </button>
              </div>
            )}
            {info.previousPath && (
              <div className="index-storage-previous">
                <h3>旧索引副本</h3>
                <p className="index-storage-path">{info.previousPath}</p>
                <p>
                  迁移已完成。旧副本仍占用原磁盘空间；确认新位置正常后，可退出
                  FileM，再清理旧位置的 index.sqlite、index.sqlite-wal 和
                  index.sqlite-shm。保留其他配置文件。
                </p>
                <button
                  className="text-button"
                  disabled={busy}
                  onClick={() => run("reveal_index", { previous: true })}
                >
                  定位旧索引
                </button>
              </div>
            )}
            <form
              className="index-storage-form"
              onSubmit={(event) => {
                event.preventDefault();
                void run(
                  "set_index_location",
                  { path },
                  "新位置已保存，重启 FileM 后开始迁移。",
                );
              }}
            >
              <label
                className="field"
                id="index-location-label"
                htmlFor="index-location"
              >
                新的存放文件夹
              </label>
              <button
                id="index-location"
                type="button"
                className={`index-directory-picker ${path ? "has-selection" : ""}`}
                aria-labelledby="index-location-label"
                title={path || "点击打开系统文件夹选择窗口"}
                disabled={busy || !info.canChange}
                onClick={browse}
              >
                <FolderOpen size={18} />
                <span>{path || "点击选择存放文件夹"}</span>
                <span className="index-directory-picker-action">
                  {path ? "更换…" : "选择…"}
                </span>
              </button>
              <p className="field-hint">
                请选择专门存放索引的空文件夹，不要选择资料文件夹或整个磁盘。已有文件列表、隐藏规则和内容索引会一并迁移；目标空间不足或不可用时，继续使用原位置并提示原因。
              </p>
              <button
                className="button primary"
                disabled={busy || !path || !info.canChange}
                type="submit"
              >
                {busy ? "正在处理…" : "保存，下次启动生效"}
              </button>
            </form>
          </>
        )}
        {message && (
          <p className="index-storage-message" role="status">
            {message}
          </p>
        )}
      </div>
      <footer className="dialog-footer">
        <button className="button outline" disabled={busy} onClick={close}>
          完成
        </button>
      </footer>
    </dialog>
  );
}
