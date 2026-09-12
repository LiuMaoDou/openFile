import { useEffect, useRef, useState } from "react";
import { FolderOpen, FolderInput, X } from "lucide-react";
import { errorText, isDesktop, pickDirectory } from "../api";
import { DeleteDialog } from "./DeleteDialog";
export function MoveDialog({
  ids,
  close,
  completed,
}: {
  ids?: string[];
  close: () => void;
  completed: (ids: string[]) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [path, setPath] = useState("");
  const [destination, setDestination] = useState<string | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    if (ids && !destination) dialog.current?.showModal();
  }, [ids, destination]);
  if (!ids || destination)
    return (
      <DeleteDialog
        action="move"
        ids={ids}
        destination={destination ?? undefined}
        close={close}
        completed={completed}
      />
    );
  return (
    <dialog
      ref={dialog}
      className="dialog"
      aria-labelledby="move-title"
      onCancel={close}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          setDestination(path);
        }}
      >
        <header className="dialog-header">
          <div>
            <h2 id="move-title">移动 {ids.length} 个文件</h2>
            <p>选择目标文件夹，下一步核对文件与同名冲突。</p>
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label="关闭移动窗口"
            onClick={close}
          >
            <X size={20} />
          </button>
        </header>
        <div className="dialog-body">
          <label className="field">
            目标文件夹
            <div className="path-input">
              <input
                autoFocus
                required
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder={
                  navigator.platform.includes("Win")
                    ? "D:\\整理后的文件"
                    : "/Users/你的用户名/Documents"
                }
              />
              {isDesktop() && (
                <button
                  type="button"
                  className="icon-button"
                  aria-label="选择目标文件夹"
                  onClick={async () => {
                    try {
                      const selected = await pickDirectory();
                      if (selected) setPath(selected);
                    } catch (e) {
                      setError(errorText(e));
                    }
                  }}
                >
                  <FolderOpen size={19} />
                </button>
              )}
            </div>
          </label>
          <p className="field-hint">
            移动会改变真实文件的位置。已有同名文件会跳过，不会覆盖。
          </p>
          {error && (
            <p className="inline-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog-footer">
          <span className="spacer" />
          <button type="button" className="button outline" onClick={close}>
            取消
          </button>
          <button className="button primary" disabled={!path.trim()}>
            <FolderInput size={16} />
            下一步：核对文件
          </button>
        </footer>
      </form>
    </dialog>
  );
}
