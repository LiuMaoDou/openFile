import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
  Cloud,
  ExternalLink,
  FolderOpen,
  LoaderCircle,
  FolderSearch,
} from "lucide-react";
import type { Entry, Query, QueryResult } from "../types";
import { FileIcon, formatDate, formatSize } from "./FileIcon";
interface Props {
  scopeCount: number;
  result: QueryResult;
  query: Query;
  change: (patch: Partial<Query>) => void;
  selected: Set<string>;
  setSelected: React.Dispatch<React.SetStateAction<Set<string>>>;
  activeId?: string;
  activate: (entry: Entry) => void;
  open: (entry: Entry, reveal?: boolean) => void;
  busyFiles: Set<string>;
  loading: boolean;
}
export function FileTable({
  scopeCount,
  result,
  query,
  change,
  selected,
  setSelected,
  activeId,
  activate,
  open,
  loading,
  busyFiles,
}: Props) {
  const scroll = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: result.entries.length,
    getScrollElement: () => scroll.current,
    estimateSize: () => 42,
    overscan: 8,
    getItemKey: (index) => result.entries[index].id,
  });
  const anchor = useRef<string | undefined>(undefined);
  const columns = [
    { key: "name", label: "文件名" },
    { key: "directory", label: "所在目录" },
    { key: "size", label: "大小" },
    { key: "mtime", label: "修改时间" },
    { key: "extension", label: "类型" },
  ];
  const allPage =
    result.entries.length > 0 &&
    result.entries.every((e) => selected.has(e.id));
  function toggle(entry: Entry, shift = false) {
    setSelected((previous) => {
      const next = new Set(previous);
      const first = result.entries.findIndex((e) => e.id === anchor.current);
      const last = result.entries.findIndex((e) => e.id === entry.id);
      if (shift && first >= 0) {
        result.entries
          .slice(Math.min(first, last), Math.max(first, last) + 1)
          .forEach((e) => next.add(e.id));
      } else {
        if (next.has(entry.id)) next.delete(entry.id);
        else next.add(entry.id);
      }
      return next;
    });
    anchor.current = entry.id;
  }
  return (
    <div className="table-region" aria-busy={loading}>
      <div className="table-scroll" ref={scroll}>
        <div
          className="file-table"
          role="table"
          aria-label="聚合文件列表"
          aria-rowcount={result.total}
        >
          <div className="table-head table-grid" role="row">
            <div role="columnheader">
              <input
                type="checkbox"
                aria-label="选择本页文件"
                checked={allPage}
                onChange={() =>
                  setSelected((previous) => {
                    const next = new Set(previous);
                    result.entries.forEach((e) =>
                      allPage ? next.delete(e.id) : next.add(e.id),
                    );
                    return next;
                  })
                }
              />
            </div>
            {columns.map((column) => (
              <div
                role="columnheader"
                key={column.key}
                aria-sort={
                  query.sort === column.key
                    ? query.descending
                      ? "descending"
                      : "ascending"
                    : "none"
                }
              >
                <button
                  onClick={() =>
                    change({
                      sort: column.key,
                      descending:
                        query.sort === column.key ? !query.descending : false,
                    })
                  }
                >
                  {column.label}
                  {query.sort === column.key &&
                    (query.descending ? (
                      <ArrowDown size={12} />
                    ) : (
                      <ArrowUp size={12} />
                    ))}
                </button>
              </div>
            ))}
            <div role="columnheader" className="row-actions">
              操作
            </div>
          </div>
          <div
            className="virtual-body"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualizer.getVirtualItems().map((item) => {
              const entry = result.entries[item.index];
              return (
                <div
                  key={entry.id}
                  role="row"
                  aria-selected={selected.has(entry.id)}
                  tabIndex={0}
                  className={`file-row table-grid ${activeId === entry.id || selected.has(entry.id) ? "selected" : ""} ${!entry.online ? "offline" : ""}`}
                  style={{
                    height: item.size,
                    transform: `translateY(${item.start}px)`,
                  }}
                  onClick={(event) => {
                    activate(entry);
                    if (event.ctrlKey || event.metaKey || event.shiftKey)
                      toggle(entry, event.shiftKey);
                  }}
                  onDoubleClick={() => open(entry)}
                  onKeyDown={(event) => {
                    if (event.target !== event.currentTarget) return;
                    if (event.key === "Enter") {
                      event.preventDefault();
                      activate(entry);
                    }
                    if (event.key === " ") {
                      event.preventDefault();
                      toggle(entry, event.shiftKey);
                    }
                  }}
                >
                  <div role="cell">
                    <input
                      type="checkbox"
                      aria-label={`选择 ${entry.name}`}
                      checked={selected.has(entry.id)}
                      onClick={(event) => event.stopPropagation()}
                      onChange={(event) =>
                        toggle(
                          entry,
                          (event.nativeEvent as MouseEvent).shiftKey,
                        )
                      }
                    />
                  </div>
                  <div className="file-name" role="cell" title={entry.name}>
                    <FileIcon
                      extension={entry.extension}
                      group={entry.group}
                      name={entry.name}
                    />
                    <span className="file-title-content">
                      <span>{entry.name}</span>
                      {entry.snippet && (
                        <small
                          className="content-snippet"
                          title={`${entry.snippet.before}${entry.snippet.matched}${entry.snippet.after}`}
                        >
                          …{entry.snippet.before}
                          <mark>{entry.snippet.matched}</mark>
                          {entry.snippet.after}…
                          {entry.snippet.truncated && "（部分内容）"}
                        </small>
                      )}
                    </span>
                    {entry.placeholder && <Cloud size={14} />}
                  </div>
                  <div role="cell" title={entry.path}>
                    {entry.directory}
                  </div>
                  <div role="cell">{formatSize(entry.size)}</div>
                  <div role="cell">{formatDate(entry.mtime)}</div>
                  <div role="cell">
                    {entry.extension
                      ? entry.extension.toUpperCase()
                      : "无扩展名"}
                  </div>
                  <div
                    role="cell"
                    className="row-actions"
                    onClick={(event) => event.stopPropagation()}
                    onDoubleClick={(event) => event.stopPropagation()}
                  >
                    <button
                      className="icon-button"
                      aria-label={`系统打开 ${entry.name}`}
                      title="系统打开"
                      disabled={
                        !entry.online ||
                        entry.placeholder ||
                        busyFiles.has(entry.id)
                      }
                      onClick={() => open(entry)}
                    >
                      {busyFiles.has(entry.id) ? (
                        <LoaderCircle className="spin" size={16} />
                      ) : (
                        <ExternalLink size={16} />
                      )}
                    </button>
                    <button
                      className="icon-button"
                      aria-label={`定位文件 ${entry.name}`}
                      title="定位文件"
                      disabled={
                        !entry.online ||
                        entry.placeholder ||
                        busyFiles.has(entry.id)
                      }
                      onClick={() => open(entry, true)}
                    >
                      <FolderOpen size={17} />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
        {result.entries.length === 0 && !loading && (
          <div className="empty-results">
            <FolderSearch size={35} strokeWidth={1.25} />
            <h3>没有找到匹配的文件</h3>
            <p>尝试其他类型、文件夹或搜索条件。</p>
            <button
              className="button outline"
              onClick={() =>
                change({
                  search: "",
                  extension: "",
                  scopeId: "",
                  group: "",
                  minSize: undefined,
                  modifiedAfter: undefined,
                })
              }
            >
              清除筛选
            </button>
          </div>
        )}
      </div>
      <div className="pagination">
        <span>
          {scopeCount.toLocaleString()} 个文件夹 ·{" "}
          {result.total.toLocaleString()} 个文件
          {result.total > query.limit
            ? ` · 第 ${Math.floor(query.offset / query.limit) + 1} / ${Math.ceil(result.total / query.limit)} 页`
            : ""}
        </span>
        <div>
          <button
            className="icon-button"
            aria-label="上一页"
            disabled={query.offset === 0 || loading}
            onClick={() => {
              change({ offset: Math.max(0, query.offset - query.limit) });
              scroll.current?.scrollTo(0, 0);
            }}
          >
            <ChevronLeft size={17} />
          </button>
          <button
            className="icon-button"
            aria-label="下一页"
            disabled={query.offset + query.limit >= result.total || loading}
            onClick={() => {
              change({ offset: query.offset + query.limit });
              scroll.current?.scrollTo(0, 0);
            }}
          >
            <ChevronRight size={17} />
          </button>
        </div>
      </div>
    </div>
  );
}
