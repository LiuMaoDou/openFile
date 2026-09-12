import { useEffect, useRef, useState } from "react";
import { FolderOpen, X } from "lucide-react";
import { command, errorText, isDesktop, pickDirectory } from "../api";
import { DEFAULT_EXCLUDES, type Scope } from "../types";
interface Props {
  scope?: Scope;
  initialPath?: string;
  close: () => void;
  changed: () => void;
  notify: (message: string) => void;
}
export function ScopeDialog({
  scope,
  initialPath = "",
  close,
  changed,
  notify,
}: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [path, setPath] = useState(scope?.path || initialPath);
  const [recursive, setRecursive] = useState(scope?.recursive ?? true);
  const [watch, setWatch] = useState(scope?.watch ?? true);
  const [excludes, setExcludes] = useState(
    (scope?.excludes || DEFAULT_EXCLUDES).join("\n"),
  );
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  async function save(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const input = {
        path,
        recursive,
        watch,
        excludes: excludes
          .split("\n")
          .map((x) => x.trim())
          .filter(Boolean),
      };
      await command(
        scope ? "update_scope" : "add_scope",
        scope ? { id: scope.id, input } : input,
      );
      changed();
      notify(
        scope
          ? "文件夹设置已更新，正在重新核对。"
          : "文件夹已添加，正在扫描文件。",
      );
      close();
    } catch (error) {
      setError(errorText(error));
      setBusy(false);
    }
  }
  async function remove() {
    setBusy(true);
    try {
      await command("remove_scope", { id: scope!.id });
      changed();
      notify("文件夹已移除，磁盘文件保持原位。");
      close();
    } catch (error) {
      setError(errorText(error));
      setBusy(false);
    }
  }
  return (
    <dialog
      ref={dialog}
      className="dialog"
      onCancel={close}
      aria-labelledby="scope-title"
    >
      <form onSubmit={save}>
        <header className="dialog-header">
          <div>
            <h2 id="scope-title">{scope ? "文件夹设置" : "添加文件夹"}</h2>
            <p>选择要聚合的文件夹，文件保留在原位置。</p>
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label="关闭文件夹设置"
            onClick={close}
          >
            <X size={20} />
          </button>
        </header>
        <div className="dialog-body">
          <label className="field">
            文件夹路径
            <div className="path-input">
              <input
                autoFocus
                required
                value={path}
                disabled={!!scope}
                onChange={(e) => setPath(e.target.value)}
                placeholder={
                  navigator.platform.includes("Win")
                    ? "D:\\Projects\\Assets"
                    : "/Users/你的用户名/Documents"
                }
              />
              {isDesktop() && !scope && (
                <button
                  type="button"
                  className="icon-button"
                  aria-label="选择文件夹"
                  onClick={async () => {
                    try {
                      const path = await pickDirectory();
                      if (path) setPath(path);
                    } catch (error) {
                      setError(errorText(error));
                    }
                  }}
                >
                  <FolderOpen size={19} />
                </button>
              )}
            </div>
          </label>
          {!isDesktop() && (
            <p className="field-hint">
              输入本机文件夹的完整路径；桌面应用支持目录选择器。
            </p>
          )}
          <label className="check-field">
            <input
              type="checkbox"
              checked={recursive}
              onChange={(e) => setRecursive(e.target.checked)}
            />
            <span>
              包含子文件夹<small>关闭后只索引根目录下的文件</small>
            </span>
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              checked={watch}
              onChange={(e) => setWatch(e.target.checked)}
            />
            <span>
              监听文件变化<small>收到变更后重新核对文件夹</small>
            </span>
          </label>
          <label className="field">
            排除规则
            <textarea
              rows={6}
              value={excludes}
              onChange={(e) => setExcludes(e.target.value)}
              spellCheck={false}
            />
          </label>
          <p className="field-hint">
            每行一条，例如
            node_modules、*.tmp、Backup/**。规则仅作用于此文件夹。
          </p>
          {scope?.message && <div className="inline-info">{scope.message}</div>}
          {error && (
            <div className="inline-error" role="alert">
              {error}
            </div>
          )}
        </div>
        <footer className="dialog-footer">
          {scope && (
            <button
              type="button"
              className="button danger-text"
              disabled={busy}
              onClick={remove}
            >
              移除文件夹
            </button>
          )}
          <span className="spacer" />
          <button type="button" className="button" onClick={close}>
            取消
          </button>
          <button
            className="button primary"
            disabled={busy || !path.trim()}
            type="submit"
          >
            {busy ? "正在处理…" : scope ? "保存设置" : "添加并扫描"}
          </button>
        </footer>
      </form>
    </dialog>
  );
}
