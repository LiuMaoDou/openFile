import { useEffect, useRef, useState } from "react";
import { Copy, EyeOff, Trash2, X } from "lucide-react";
import { command, errorText } from "../api";
import type { Entry } from "../types";
import { FileIcon, formatDate, formatSize } from "./FileIcon";
export function Inspector({
  entry,
  search,
  close,
  notify,
  fail,
  remove,
  hideDirectory,
}: {
  entry: Entry;
  search: string;
  close: () => void;
  notify: (text: string) => void;
  fail: (text: string) => void;
  remove: () => void;
  hideDirectory: () => void;
}) {
  const [preview, setPreview] = useState<{
    text: string;
    truncated: boolean;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const request = useRef(0);
  useEffect(() => {
    request.current++;
    setPreview(null);
    setLoading(false);
  }, [entry.id, entry.mtime, entry.size, entry.online, search]);
  const textType = [
    "txt",
    "md",
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
    "json",
    "toml",
    "yaml",
    "yml",
    "csv",
    "log",
    "css",
    "html",
    "py",
    "go",
    "c",
    "cpp",
    "h",
    "sh",
  ].includes(entry.extension);
  const indexed = entry.contentReady;
  async function loadPreview() {
    const sequence = ++request.current;
    setLoading(true);
    try {
      const result = await command<{ text: string; truncated: boolean }>(
        indexed ? "content_preview" : "preview_text",
        { id: entry.id, search },
      );
      if (request.current === sequence) setPreview(result);
    } catch (error) {
      fail(errorText(error));
    } finally {
      if (request.current === sequence) setLoading(false);
    }
  }
  return (
    <aside className="inspector" aria-label="文件详情">
      <header>
        <h2>文件详情</h2>
        <button
          className="icon-button"
          aria-label="关闭文件详情"
          onClick={close}
        >
          <X size={20} />
        </button>
      </header>
      <div className="file-illustration">
        <FileIcon
          extension={entry.extension}
          group={entry.group}
          name={entry.name}
          large
        />
      </div>
      <h3>{entry.name}</h3>
      <dl>
        <dt>类型</dt>
        <dd>
          {entry.extension.toUpperCase() || "无扩展名"} · {entry.group}
        </dd>
        <dt>所在目录</dt>
        <dd title={entry.path}>{entry.directory}</dd>
        <dt>大小</dt>
        <dd>{formatSize(entry.size)}</dd>
        <dt>修改时间</dt>
        <dd>{formatDate(entry.mtime)}</dd>
        <dt>所属文件夹</dt>
        <dd>{entry.scopeName}</dd>
      </dl>
      {entry.directoryHidden && (
        <p className="inline-info">
          此文件随目录隐藏，点击下方“管理目录隐藏”可恢复对应目录。
        </p>
      )}
      {(!entry.online || entry.placeholder) && (
        <p className="inline-info">
          {entry.placeholder
            ? "云占位文件，当前版本不自动读取内容。"
            : "所在文件夹不可用，正在显示上次索引。"}
        </p>
      )}
      <div className="inspector-actions" role="group" aria-label="文件操作">
        <button
          className="button primary"
          disabled={
            !entry.online ||
            entry.placeholder ||
            (!textType && !indexed) ||
            loading
          }
          onClick={loadPreview}
        >
          {loading ? "读取中…" : "预览文本"}
        </button>
        <button
          className="button outline"
          onClick={() =>
            navigator.clipboard
              .writeText(entry.path)
              .then(() => notify("完整路径已复制。"))
              .catch((error) => fail(errorText(error)))
          }
        >
          <Copy size={15} />
          复制路径
        </button>
        <button className="button outline" onClick={hideDirectory}>
          <EyeOff size={15} />
          {entry.directoryHidden ? "管理目录隐藏" : "隐藏所在目录"}
        </button>
        <button
          className="button outline danger-text"
          disabled={!entry.online || entry.placeholder}
          onClick={remove}
        >
          <Trash2 size={15} />
          删除此文件
        </button>
      </div>
      <p className="field-hint">目录隐藏包含所在目录及其所有子目录的文件。</p>
      {!textType && !indexed && (
        <p className="field-hint">
          此格式暂不提供内置预览，可使用系统应用打开。
        </p>
      )}
      {preview && (
        <div className="text-preview">
          <header>
            <span>文本预览</span>
            <button
              className="icon-button"
              aria-label="关闭文本预览"
              onClick={() => setPreview(null)}
            >
              <X size={15} />
            </button>
          </header>
          <pre>{preview.text}</pre>
          {preview.truncated && (
            <p>仅显示部分文字；搜索时优先展示命中位置附近的内容。</p>
          )}
        </div>
      )}
    </aside>
  );
}
