import { useEffect, useRef, useState } from "react";
import { Eye, EyeOff, FolderOpen, X } from "lucide-react";
import { command, errorText, isDesktop, pickDirectory } from "../api";
import type { HiddenDirectory, Scope } from "../types";

interface Props {
  scopes: Scope[];
  rules: HiddenDirectory[];
  initialPath: string;
  close: () => void;
  changed: () => void;
  notify: (message: string) => void;
}

export function HiddenDirectoriesDialog({
  scopes,
  rules,
  initialPath,
  close,
  changed,
  notify,
}: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [path, setPath] = useState(initialPath);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    dialog.current?.showModal();
  }, []);

  async function hide(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await command("hide_directory", { path });
      changed();
      notify("已隐藏该目录及所有子目录的内容，新增文件也会自动隐藏。");
      setPath("");
    } catch (error) {
      setError(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  async function restore(rule: HiddenDirectory) {
    setBusy(true);
    setError("");
    try {
      await command("restore_directory", { id: rule.id });
      changed();
      notify(
        "已移除该目录的隐藏规则；单独隐藏或受其他目录规则影响的文件仍保留隐藏。",
      );
    } catch (error) {
      setError(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialog}
      className="dialog hidden-directories-dialog"
      aria-labelledby="hidden-directories-title"
      onCancel={close}
    >
      <header className="dialog-header">
        <div>
          <h2 id="hidden-directories-title">隐藏目录</h2>
          <p>在 FileM 中隐藏目录内容，包含所有子目录和以后新增的文件。</p>
        </div>
        <button
          className="icon-button"
          aria-label="关闭隐藏目录"
          onClick={close}
        >
          <X size={20} />
        </button>
      </header>
      <div className="dialog-body hidden-directories-body">
        <form onSubmit={hide}>
          <label className="field" htmlFor="hidden-scope">
            选择监控文件夹
          </label>
          <select
            id="hidden-scope"
            className="hidden-scope-select"
            value={scopes.some((scope) => scope.path === path) ? path : ""}
            disabled={busy}
            onChange={(event) => setPath(event.target.value)}
          >
            <option value="">选择一个文件夹，或在下方指定子目录</option>
            {scopes.map((scope) => (
              <option key={scope.id} value={scope.path}>
                {scope.path}
              </option>
            ))}
          </select>
          <label className="field" htmlFor="hidden-directory-path">
            要隐藏的目录路径
          </label>
          <div className="path-input">
            <input
              id="hidden-directory-path"
              autoFocus
              required
              value={path}
              disabled={busy}
              onChange={(event) => setPath(event.target.value)}
              placeholder="输入目录的完整路径"
            />
            {isDesktop() && (
              <button
                type="button"
                className="icon-button"
                aria-label="选择要隐藏的目录"
                disabled={busy}
                onClick={async () => {
                  try {
                    const selected = await pickDirectory();
                    if (selected) setPath(selected);
                  } catch (error) {
                    setError(errorText(error));
                  }
                }}
              >
                <FolderOpen size={19} />
              </button>
            )}
          </div>
          <p className="field-hint">
            {isDesktop()
              ? "点击文件夹图标选择目录。"
              : "浏览器版可输入子目录路径；桌面应用支持目录选择器。"}
            隐藏范围包含该目录中的全部已索引文件，不受当前搜索和类型筛选限制。
          </p>
          {error && (
            <div className="inline-error" role="alert">
              {error}
            </div>
          )}
          <button
            className="button primary hide-directory-submit"
            type="submit"
            disabled={busy || !path.trim()}
          >
            <EyeOff size={16} />
            {busy ? "正在处理…" : "隐藏目录内容"}
          </button>
        </form>
        <section className="hidden-directory-rules" aria-label="已隐藏的目录">
          <h3>
            已隐藏的目录 <small>{rules.length}</small>
          </h3>
          {rules.length === 0 ? (
            <p className="field-hint">尚未设置目录隐藏规则。</p>
          ) : (
            <>
              <ul>
                {rules.map((rule) => (
                  <li key={rule.id}>
                    <span title={rule.path}>{rule.path}</span>
                    <button
                      className="button outline"
                      disabled={busy}
                      onClick={() => restore(rule)}
                      aria-label={`恢复目录 ${rule.path}`}
                    >
                      <Eye size={15} />
                      恢复显示
                    </button>
                  </li>
                ))}
              </ul>
              <p className="field-hint">
                恢复目录后，单独隐藏的文件和其他目录隐藏规则继续生效。
              </p>
            </>
          )}
        </section>
      </div>
      <footer className="dialog-footer">
        <span className="spacer" />
        <button className="button" onClick={close}>
          完成
        </button>
      </footer>
    </dialog>
  );
}
