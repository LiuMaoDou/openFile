import { useEffect, useRef, useState, type CSSProperties } from "react";
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
import {
  FILE_COLUMNS,
  FILE_COLUMN_STORAGE_KEY,
  MAX_FILE_COLUMN_WIDTH,
  defaultFileColumnLayout,
  fileColumnGridTemplate,
  fileColumnMinTableWidth,
  moveFileColumn,
  parseFileColumnLayout,
  setFileColumnWidth,
  type FileColumnKey,
  type FileColumnLayout,
} from "./fileTableColumns";
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
  const [columnLayout, setColumnLayout] = useState<FileColumnLayout>(() => {
    try {
      return parseFileColumnLayout(
        window.localStorage.getItem(FILE_COLUMN_STORAGE_KEY),
      );
    } catch {
      return defaultFileColumnLayout();
    }
  });
  const [draggedColumn, setDraggedColumn] = useState<FileColumnKey | null>(null);
  const draggingColumn = useRef<FileColumnKey | null>(null);
  const [dropTarget, setDropTarget] = useState<{
    key: FileColumnKey;
    side: "before" | "after";
  } | null>(null);
  const resizing = useRef<{
    key: FileColumnKey;
    pointerId: number;
    startX: number;
    startWidth: number;
  } | null>(null);
  useEffect(() => {
    try {
      window.localStorage.setItem(
        FILE_COLUMN_STORAGE_KEY,
        JSON.stringify(columnLayout),
      );
    } catch {
      // Private browsing or disabled storage must not break the table.
    }
  }, [columnLayout]);
  const columns = columnLayout.order.map(
    (key) => FILE_COLUMNS.find((column) => column.key === key)!,
  );
  const tableStyle = {
    minWidth: fileColumnMinTableWidth(columnLayout),
    "--file-table-columns": fileColumnGridTemplate(columnLayout),
  } as CSSProperties;
  function onColumnDragStart(
    event: React.DragEvent<HTMLDivElement>,
    key: FileColumnKey,
  ) {
    if (resizing.current) {
      event.preventDefault();
      return;
    }
    draggingColumn.current = key;
    setDraggedColumn(key);
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", key);
  }
  function onColumnDragOver(
    event: React.DragEvent<HTMLDivElement>,
    key: FileColumnKey,
  ) {
    const source = draggingColumn.current;
    if (!source || source === key) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    const rect = event.currentTarget.getBoundingClientRect();
    const side = event.clientX < rect.left + rect.width / 2 ? "before" : "after";
    setDropTarget((previous) =>
      previous?.key === key && previous.side === side
        ? previous
        : { key, side },
    );
  }
  function onColumnDrop(
    event: React.DragEvent<HTMLDivElement>,
    key: FileColumnKey,
  ) {
    const source = draggingColumn.current;
    if (source && source !== key) {
      event.preventDefault();
      const rect = event.currentTarget.getBoundingClientRect();
      const side = event.clientX < rect.left + rect.width / 2 ? "before" : "after";
      setColumnLayout((previous) => moveFileColumn(previous, source, key, side));
    }
    onColumnDragEnd();
  }
  function onColumnDragEnd() {
    draggingColumn.current = null;
    setDraggedColumn(null);
    setDropTarget(null);
  }
  function onResizeStart(
    event: React.PointerEvent<HTMLSpanElement>,
    key: FileColumnKey,
  ) {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const header = event.currentTarget.parentElement;
    const fallback = FILE_COLUMNS.find((column) => column.key === key)!;
    resizing.current = {
      key,
      pointerId: event.pointerId,
      startX: event.clientX,
      startWidth: header?.getBoundingClientRect().width ?? fallback.defaultWidth,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  }
  function onResizeMove(
    event: React.PointerEvent<HTMLSpanElement>,
    key: FileColumnKey,
  ) {
    const current = resizing.current;
    if (!current || current.key !== key || current.pointerId !== event.pointerId)
      return;
    setColumnLayout((previous) =>
      setFileColumnWidth(
        previous,
        key,
        current.startWidth + event.clientX - current.startX,
      ),
    );
  }
  function onResizeEnd(event: React.PointerEvent<HTMLSpanElement>) {
    if (resizing.current?.pointerId !== event.pointerId) return;
    resizing.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }
  function renderFileColumn(entry: Entry, key: FileColumnKey) {
    switch (key) {
      case "name":
        return (
          <div className="file-name" role="cell" title={entry.name} key={key}>
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
        );
      case "directory":
        return (
          <div role="cell" title={entry.path} key={key}>
            {entry.directory}
          </div>
        );
      case "size":
        return (
          <div role="cell" key={key}>
            {formatSize(entry.size)}
          </div>
        );
      case "mtime":
        return (
          <div role="cell" key={key}>
            {formatDate(entry.mtime)}
          </div>
        );
      case "extension":
        return (
          <div role="cell" key={key}>
            {entry.extension ? entry.extension.toUpperCase() : "无扩展名"}
          </div>
        );
    }
  }
  const allPage =
    result.entries.length > 0 &&
    result.entries.every((e) => selected.has(e.id));
  function toggle(entry: Entry, shift = false) {
    // State updaters may run after the event has updated the selection anchor.
    const first = result.entries.findIndex((e) => e.id === anchor.current);
    const last = result.entries.findIndex((e) => e.id === entry.id);
    setSelected((previous) => {
      const next = new Set(previous);
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
          style={tableStyle}
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
                className={[
                  "file-column-header",
                  draggedColumn === column.key ? "dragging" : "",
                  dropTarget?.key === column.key
                    ? `drop-${dropTarget.side}`
                    : "",
                ].filter(Boolean).join(" ")}
                draggable
                title="拖动列头调整顺序；按 Alt + 左右方向键移动"
                onDragStart={(event) => onColumnDragStart(event, column.key)}
                onDragOver={(event) => onColumnDragOver(event, column.key)}
                onDrop={(event) => onColumnDrop(event, column.key)}
                onDragEnd={onColumnDragEnd}
                onKeyDown={(event) => {
                  if (
                    !event.altKey ||
                    (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
                  ) return;
                  event.preventDefault();
                  const index = columnLayout.order.indexOf(column.key);
                  const targetIndex =
                    index + (event.key === "ArrowLeft" ? -1 : 1);
                  const target = columnLayout.order[targetIndex];
                  if (!target) return;
                  setColumnLayout((previous) =>
                    moveFileColumn(
                      previous,
                      column.key,
                      target,
                      event.key === "ArrowLeft" ? "before" : "after",
                    ),
                  );
                }}
                aria-sort={
                  query.sort === column.key
                    ? query.descending
                      ? "descending"
                      : "ascending"
                    : "none"
                }
              >
                <button
                  type="button"
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
                <span
                  className="column-resizer"
                  role="separator"
                  tabIndex={0}
                  draggable={false}
                  aria-label={`调整${column.label}列宽`}
                  aria-orientation="vertical"
                  aria-valuemin={column.minWidth}
                  aria-valuemax={MAX_FILE_COLUMN_WIDTH}
                  aria-valuenow={
                    columnLayout.widths[column.key] ?? column.defaultWidth
                  }
                  title="拖动调整列宽；方向键微调（Shift 加速）"
                  onPointerDown={(event) => onResizeStart(event, column.key)}
                  onPointerMove={(event) => onResizeMove(event, column.key)}
                  onPointerUp={onResizeEnd}
                  onPointerCancel={onResizeEnd}
                  onLostPointerCapture={() => {
                    resizing.current = null;
                  }}
                  onDragStart={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                  }}
                  onKeyDown={(event) => {
                    if (
                      event.key !== "ArrowLeft" &&
                      event.key !== "ArrowRight"
                    ) return;
                    event.preventDefault();
                    event.stopPropagation();
                    const currentWidth =
                      columnLayout.widths[column.key] ??
                      event.currentTarget.parentElement?.getBoundingClientRect()
                        .width ??
                      column.defaultWidth;
                    const delta =
                      (event.key === "ArrowLeft" ? -1 : 1) *
                      (event.shiftKey ? 32 : 16);
                    setColumnLayout((previous) =>
                      setFileColumnWidth(
                        previous,
                        column.key,
                        currentWidth + delta,
                      ),
                    );
                  }}
                />
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
                  {columnLayout.order.map((key) =>
                    renderFileColumn(entry, key),
                  )}
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
