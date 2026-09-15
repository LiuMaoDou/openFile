import { useCallback, useEffect, useRef, useState } from "react";
import {
  Check,
  Eye,
  EyeOff,
  FolderPlus,
  FolderInput,
  Menu,
  RefreshCw,
  Search,
  SlidersHorizontal,
  Trash2,
  X,
} from "lucide-react";
import { command, errorText } from "./api";
import { DEFAULT_QUERY, type Entry, type Query, type Scope } from "./types";
import { useWorkspace } from "./useWorkspace";
import { Sidebar } from "./components/Sidebar";
import { ScopeDialog } from "./components/ScopeDialog";
import { Inspector } from "./components/Inspector";
import { FileTable } from "./components/FileTable";
import { ContentIndex } from "./components/ContentIndex";
import { SearchBackend } from "./components/SearchBackend";
import { MoveDialog } from "./components/MoveDialog";
import { ScanStatus } from "./components/ScanStatus";
import { DeleteDialog } from "./components/DeleteDialog";
import { HiddenDirectoriesDialog } from "./components/HiddenDirectoriesDialog";

export default function App() {
  const [query, setQuery] = useState<Query>(DEFAULT_QUERY);
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [active, setActive] = useState<Entry | null>(null);
  const [dialog, setDialog] = useState<{ scope?: Scope } | null>(null);
  const [hiddenDirectoryDialog, setHiddenDirectoryDialog] = useState<{
    path: string;
  } | null>(null);
  const [moving, setMoving] = useState<{ ids?: string[] } | null>(null);
  const [deleting, setDeleting] = useState<{ ids?: string[] } | null>(null);
  const [mobileOpen, setMobileOpen] = useState(false);
  const [filters, setFilters] = useState(false);
  const [toast, setToast] = useState("");
  const opening = useRef(new Set<string>());
  const [busyFiles, setBusyFiles] = useState(new Set<string>());
  const [working, setWorking] = useState(false);
  const { summary, result, connected, loading, error, setError, refresh } =
    useWorkspace(query);
  const notify = useCallback((message: string) => setToast(message), []);
  const change = useCallback((patch: Partial<Query>) => {
    setQuery((previous) => ({
      ...previous,
      ...(patch.scopeId !== undefined ? { group: "", extension: "" } : {}),
      offset: 0,
      ...patch,
    }));
    if (patch.search !== undefined) setSearch(patch.search);
  }, []);
  useEffect(() => {
    const timer = setTimeout(
      () =>
        setQuery((previous) =>
          previous.search === search
            ? previous
            : { ...previous, search, offset: 0 },
        ),
      200,
    );
    return () => clearTimeout(timer);
  }, [search]);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(""), 4200);
    return () => clearTimeout(timer);
  }, [toast]);
  useEffect(() => {
    if (active) {
      const updated = result.entries.find((e) => e.id === active.id);
      setActive(updated ?? null);
    }
  }, [result]);
  useEffect(() => {
    if (
      !loading &&
      result.offset === query.offset &&
      query.offset > 0 &&
      query.offset >= result.total
    )
      change({
        offset:
          Math.max(0, Math.ceil(result.total / query.limit) - 1) * query.limit,
      });
  }, [loading, result, query.offset, query.limit, change]);
  const scanning = summary.scopes.filter((s) =>
    ["scanning", "verifying", "dirty"].includes(s.freshness),
  );
  const title = query.hidden
    ? "已隐藏"
    : query.group ||
      (query.scopeId
        ? summary.scopes.find((s) => s.id === query.scopeId)?.name
        : "") ||
      "全部文件";
  const facetsReady =
    summary.facetScopeId === query.scopeId &&
    summary.facetHidden === query.hidden;
  const extensions = !facetsReady
    ? []
    : query.hidden
      ? (summary.hiddenExtensions ?? [])
      : summary.extensions;
  const selectedActive = active;
  async function execute(name: string, args: unknown = {}, message?: string) {
    setWorking(true);
    try {
      await command(name, args);
      refresh();
      if (message) notify(message);
      return true;
    } catch (error) {
      setError(errorText(error));
      return false;
    } finally {
      setWorking(false);
    }
  }
  async function openFile(entry: Entry, reveal = false) {
    if (!entry.online || entry.placeholder || opening.current.has(entry.id))
      return;
    opening.current.add(entry.id);
    setBusyFiles(new Set(opening.current));
    try {
      await command("open_entry", { id: entry.id, reveal });
    } catch (error) {
      setError(`${entry.name}：${errorText(error)}`);
    } finally {
      opening.current.delete(entry.id);
      setBusyFiles(new Set(opening.current));
    }
  }
  async function hide() {
    if (
      await execute(
        "set_hidden",
        { ids: [...selected], hidden: !query.hidden },
        query.hidden
          ? "文件已恢复显示。"
          : "已从视图隐藏，可在“已隐藏”中恢复。",
      )
    ) {
      setSelected(new Set());
      setActive(null);
    }
  }
  function changedScopes() {
    refresh();
    setSelected(new Set());
    setActive(null);
    change({ scopeId: "" });
  }
  return (
    <div className="app-shell">
      {mobileOpen && (
        <button
          className="nav-backdrop"
          aria-label="关闭导航遮罩"
          onClick={() => setMobileOpen(false)}
        />
      )}
      <Sidebar
        summary={summary}
        query={query}
        change={change}
        add={() => setDialog({})}
        edit={(scope) => setDialog({ scope })}
        mobileOpen={mobileOpen}
        close={() => setMobileOpen(false)}
      />
      <div className="workspace">
        <div className="content-row">
          <main>
            <header className="workspace-header">
              <div className="heading">
                <button
                  className="icon-button mobile-menu"
                  aria-label="展开导航"
                  onClick={() => setMobileOpen(true)}
                >
                  <Menu size={23} />
                </button>
                <div className="heading-content">
                  <h1 title={title}>{title}</h1>
                  <div className="workspace-status">
                    <span className="workspace-status-message" role="status">
                      <i
                        className={`status-dot ${!connected ? "offline" : scanning.length ? "busy" : ""}`}
                      />
                      <span>
                        {!connected
                          ? "正在连接本地索引服务"
                          : summary.scanPaused
                            ? "文件操作期间暂停扫描 · 完成后自动继续"
                            : scanning.length
                              ? `正在核对 ${scanning.length} 个文件夹 · 已发现 ${scanning.reduce((n, s) => n + s.scanned, 0).toLocaleString()} 个文件`
                              : !summary.scopes.length
                                ? "尚未添加文件夹"
                                : summary.scopes.some(
                                      (s) =>
                                        s.availability !== "available" ||
                                        s.freshness === "partial",
                                    )
                                  ? "部分文件夹需要处理，请查看文件夹设置"
                                  : "已完成扫描"}
                      </span>
                    </span>
                    {scanning.length > 0 && (
                      <button
                        className="text-button"
                        onClick={() =>
                          scanning.forEach(
                            (scope) => void execute("cancel", { id: scope.id }),
                          )
                        }
                      >
                        取消扫描
                      </button>
                    )}
                  </div>
                </div>
              </div>
            </header>
            <div className="toolbar">
              <select
                aria-label="搜索模式"
                value={query.searchMode}
                onChange={(e) =>
                  change({ searchMode: e.target.value as Query["searchMode"] })
                }
              >
                <option value="name">文件名 / 路径</option>
                <option value="content">文件内容</option>
                <option value="all">文件名 + 内容</option>
              </select>
              <label className="search-field">
                <Search size={18} />
                <input
                  aria-label="搜索关键词"
                  maxLength={512}
                  placeholder={
                    query.searchMode === "name"
                      ? "搜索文件名或路径"
                      : "输入内容关键词或完整短语"
                  }
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
                {search && (
                  <button
                    className="icon-button"
                    aria-label="清除搜索"
                    onClick={() => setSearch("")}
                  >
                    <X size={15} />
                  </button>
                )}
              </label>
              <select
                aria-label="文件夹筛选"
                value={query.scopeId}
                onChange={(e) => change({ scopeId: e.target.value })}
              >
                <option value="">文件夹：全部</option>
                {summary.scopes.map((scope) => (
                  <option key={scope.id} value={scope.id}>
                    {scope.name}
                  </option>
                ))}
              </select>
              <button
                className="button outline"
                disabled={working || !summary.scopes.length}
                onClick={() =>
                  execute(
                    "refresh",
                    query.scopeId ? { id: query.scopeId } : {},
                    "正在重新核对文件。",
                  )
                }
              >
                <RefreshCw
                  size={17}
                  className={scanning.length ? "spin" : ""}
                />
                刷新
              </button>
              <button
                className={`icon-button filter-toggle ${filters ? "active" : ""}`}
                aria-label="更多筛选"
                aria-expanded={filters}
                onClick={() => setFilters(!filters)}
              >
                <SlidersHorizontal size={18} />
              </button>
            </div>
            {filters && (
              <div className="extra-filters">
                <label>
                  文件大小
                  <select
                    aria-label="文件大小筛选"
                    value={query.minSize || ""}
                    onChange={(e) =>
                      change({
                        minSize: e.target.value
                          ? Number(e.target.value)
                          : undefined,
                      })
                    }
                  >
                    <option value="">不限</option>
                    <option value="1048576">至少 1 MB</option>
                    <option value="10485760">至少 10 MB</option>
                    <option value="104857600">至少 100 MB</option>
                  </select>
                </label>
                <label>
                  修改时间
                  <select
                    aria-label="修改时间筛选"
                    value={query.modifiedAfter ? "30" : ""}
                    onChange={(e) =>
                      change({
                        modifiedAfter: e.target.value
                          ? Date.now() - 30 * 86400000
                          : undefined,
                      })
                    }
                  >
                    <option value="">不限</option>
                    <option value="30">最近 30 天</option>
                  </select>
                </label>
              </div>
            )}
            {query.searchMode === "name" ? (
              <SearchBackend changed={refresh} result={result} />
            ) : (
              <p className="content-search-notice" role="status">
                {result.searchNotice ||
                  "搜索已索引的文档文字；请在下方开启内容索引"}
              </p>
            )}
            {summary.scopes.length > 0 && (
              <ContentIndex
                scopeId={query.scopeId}
                changed={refresh}
                fail={setError}
              />
            )}
            <ScanStatus
              scopes={summary.scopes}
              runs={summary.scanRuns}
              paused={summary.scanPaused}
            />
            <div className="extension-strip">
              <button
                className={`extension-chip ${!query.extension ? "active" : ""}`}
                onClick={() => change({ extension: "" })}
              >
                全部
              </button>
              {extensions.slice(0, 12).map((ext) => (
                <button
                  key={ext.name}
                  className={`extension-chip ${query.extension === (ext.name || "__none__") ? "active" : ""}`}
                  onClick={() =>
                    change({ extension: ext.name || "__none__", group: "" })
                  }
                >
                  {ext.name ? `.${ext.name}` : "无扩展名"}
                  <small>{ext.count}</small>
                </button>
              ))}
              {extensions.length > 12 && (
                <select
                  aria-label="更多扩展名"
                  value={query.extension}
                  onChange={(e) =>
                    change({ extension: e.target.value, group: "" })
                  }
                >
                  <option value="">更多后缀</option>
                  {extensions.map((ext) => (
                    <option value={ext.name || "__none__"} key={ext.name}>
                      {ext.name ? `.${ext.name}` : "无扩展名"} ({ext.count})
                    </option>
                  ))}
                </select>
              )}
            </div>
            {error && (
              <div className="error-banner" role="alert">
                <span>{error}</span>
                <button
                  className="icon-button"
                  aria-label="关闭错误提示"
                  onClick={() => setError("")}
                >
                  <X size={17} />
                </button>
              </div>
            )}
            {summary.scopes.length === 0 && connected ? (
              <div className="welcome">
                <div className="welcome-symbol">
                  <FolderPlus size={48} strokeWidth={1.2} />
                </div>
                <h2>文件在原位，整理从这里开始</h2>
                <p>
                  从左侧栏添加文件夹，把分散在不同目录的同类文件
                  <br />
                  聚合到同一个视图。
                </p>
                <button
                  className="text-button demo-button"
                  disabled={working}
                  onClick={() =>
                    execute("add_demo", {}, "已创建并添加独立的示例资料。")
                  }
                >
                  先体验示例资料
                </button>
                <small>由你选择文件夹，FileM 才开始索引</small>
              </div>
            ) : (
              <FileTable
                scopeCount={
                  summary.scopes.filter(
                    (scope) => !query.scopeId || scope.id === query.scopeId,
                  ).length
                }
                result={result}
                query={query}
                change={change}
                selected={selected}
                setSelected={setSelected}
                activeId={active?.id}
                activate={setActive}
                open={openFile}
                busyFiles={busyFiles}
                loading={loading}
              />
            )}
            {query.hidden && summary.hiddenDirectories.length > 0 && (
              <div className="hidden-directory-notice">
                <span>
                  随目录隐藏的文件，请通过“管理目录规则”恢复对应目录。
                </span>
                <button
                  className="text-button"
                  onClick={() => setHiddenDirectoryDialog({ path: "" })}
                >
                  管理目录规则
                </button>
              </div>
            )}
            {selected.size > 0 && (
              <div className="selection-bar">
                <span>
                  已选择 <strong>{selected.size}</strong> 个文件
                </span>
                <button
                  className="button outline"
                  disabled={working}
                  onClick={hide}
                >
                  {query.hidden ? <Eye size={15} /> : <EyeOff size={15} />}
                  {query.hidden ? "恢复显示" : "从视图隐藏"}
                </button>
                <button
                  className="button outline"
                  disabled={working}
                  onClick={() => setMoving({ ids: [...selected] })}
                >
                  <FolderInput size={15} />
                  移动到…
                </button>
                <button
                  className="text-button"
                  onClick={() => setSelected(new Set())}
                >
                  取消选择
                </button>
                <button
                  className="button outline danger-text"
                  disabled={working}
                  onClick={() => setDeleting({ ids: [...selected] })}
                >
                  <Trash2 size={15} />
                  删除文件
                </button>
              </div>
            )}
          </main>
          {selectedActive && (
            <Inspector
              search={query.search}
              key={selectedActive.id}
              entry={selectedActive}
              close={() => setActive(null)}
              notify={notify}
              fail={setError}
              remove={() => setDeleting({ ids: [selectedActive.id] })}
              hideDirectory={() =>
                setHiddenDirectoryDialog({ path: selectedActive.directoryPath })
              }
            />
          )}
        </div>
      </div>
      {dialog && (
        <ScopeDialog
          scope={dialog.scope}
          close={() => setDialog(null)}
          changed={changedScopes}
          notify={notify}
        />
      )}
      {hiddenDirectoryDialog && (
        <HiddenDirectoriesDialog
          scopes={summary.scopes}
          rules={summary.hiddenDirectories}
          initialPath={hiddenDirectoryDialog.path}
          close={() => setHiddenDirectoryDialog(null)}
          changed={() => {
            refresh();
            setSelected(new Set());
            setActive(null);
            change({ offset: 0 });
          }}
          notify={notify}
        />
      )}
      {toast && (
        <div className="toast" role="status">
          <Check size={17} />
          {toast}
        </div>
      )}
      {moving && (
        <MoveDialog
          ids={moving.ids}
          close={() => setMoving(null)}
          completed={(ids) => {
            const moved = new Set(ids);
            setSelected(
              (previous) =>
                new Set([...previous].filter((id) => !moved.has(id))),
            );
            setActive((previous) =>
              previous && moved.has(previous.id) ? null : previous,
            );
            refresh();
          }}
        />
      )}
      {deleting && (
        <DeleteDialog
          ids={deleting.ids}
          close={() => setDeleting(null)}
          completed={(ids) => {
            const removed = new Set(ids);
            setSelected(
              (previous) =>
                new Set([...previous].filter((id) => !removed.has(id))),
            );
            setActive((previous) =>
              previous && removed.has(previous.id) ? null : previous,
            );
            refresh();
          }}
        />
      )}
    </div>
  );
}
