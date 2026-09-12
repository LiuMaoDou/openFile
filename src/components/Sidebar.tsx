import {
  Archive,
  Braces,
  Check,
  ChevronRight,
  EyeOff,
  FileText,
  Film,
  Folder,
  Image,
  MoreHorizontal,
  PencilRuler,
  Plus,
  Settings2,
  X,
} from "lucide-react";
import { scanLabel } from "./ScanStatus";
import type { Query, Scope, Summary } from "../types";
const groups = [
  { name: "图片", Icon: Image },
  { name: "设计源文件", Icon: PencilRuler },
  { name: "文档", Icon: FileText },
  { name: "影音", Icon: Film },
  { name: "代码", Icon: Braces },
  { name: "压缩包", Icon: Archive },
  { name: "RAW", Icon: Image },
  { name: "其他", Icon: MoreHorizontal },
];
const scopeState: Record<string, string> = {
  available: "可用",
  offline: "离线",
  access_denied: "没有访问权限",
  identity_changed: "目录身份变化",
  unscanned: "等待扫描",
  scanning: "扫描中",
  verifying: "核对中",
  dirty: "有待核对变更",
  current: "已更新",
  partial: "部分结果待核对",
};
interface Props {
  summary: Summary;
  query: Query;
  change: (patch: Partial<Query>) => void;
  add: () => void;
  edit: (scope: Scope) => void;
  mobileOpen: boolean;
  close: () => void;
}
export function Sidebar({
  summary,
  query,
  change,
  add,
  edit,
  mobileOpen,
  close,
}: Props) {
  const navigate = (patch: Partial<Query>) => {
    change(patch);
    close();
  };
  return (
    <aside
      className={`sidebar ${mobileOpen ? "mobile-open" : ""}`}
      aria-label="文件导航"
    >
      <div className="brand">
        <Folder size={31} strokeWidth={1.7} />
        <span>FileM</span>
        <button
          className="icon-button mobile-close"
          aria-label="关闭导航"
          onClick={close}
        >
          <X size={18} />
        </button>
      </div>
      <nav>
        <section className="nav-section">
          <h2>文件库</h2>
          <button
            className={`nav-item ${!query.hidden && !query.group && !query.scopeId ? "active" : ""}`}
            onClick={() =>
              navigate({ hidden: false, group: "", scopeId: "", extension: "" })
            }
          >
            <Folder size={20} />
            <span>全部文件</span>
            <small>{summary.total.toLocaleString()}</small>
          </button>
          <button
            className={`nav-item ${query.hidden ? "active" : ""}`}
            onClick={() =>
              navigate({ hidden: true, group: "", scopeId: "", extension: "" })
            }
          >
            <EyeOff size={19} />
            <span>已隐藏</span>
            {summary.hidden > 0 && <small>{summary.hidden}</small>}
          </button>
        </section>
        <section className="nav-section">
          <h2>监控文件夹</h2>
          {summary.scopes.length === 0 && (
            <p className="nav-empty">从一个文件夹开始</p>
          )}
          {summary.scopes.map((scope) => (
            <div
              className={`scope-row ${query.scopeId === scope.id ? "active" : ""}`}
              key={scope.id}
            >
              <button
                className="scope-link"
                title={scope.path}
                onClick={() => navigate({ scopeId: scope.id, hidden: false })}
              >
                <Folder size={18} />
                <span className="scope-name-status">
                  <span>{scope.name}</span>
                  <small>
                    {scanLabel(scope, summary.scanPaused)} ·{" "}
                    {scope.count.toLocaleString()} 个
                  </small>
                </span>
                <i
                  className={`status-dot ${scope.availability !== "available" ? "offline" : ""} ${!summary.scanPaused && ["scanning", "dirty", "verifying"].includes(scope.freshness) ? "busy" : ""}`}
                  title={`${scopeState[scope.availability] || "待核对"} · ${scopeState[scope.freshness] || "待核对"}`}
                />
              </button>
              <button
                className="scope-settings icon-button"
                aria-label={`设置文件夹 ${scope.name}`}
                onClick={() => edit(scope)}
              >
                <Settings2 size={15} />
              </button>
            </div>
          ))}
          <button className="button outline add-scope-side" onClick={add}>
            <Plus size={16} />
            添加文件夹
          </button>
        </section>
        <section className="nav-section types">
          <h2>文件类型</h2>
          {summary.facetScopeId !== query.scopeId ||
          summary.facetHidden !== query.hidden ? (
            <p className="nav-empty">正在统计…</p>
          ) : summary.groups.length === 0 ? (
            <p className="nav-empty">此文件夹暂无文件</p>
          ) : null}
          {groups
            .filter(
              (g) =>
                summary.facetScopeId === query.scopeId &&
                summary.facetHidden === query.hidden &&
                summary.groups.some((b) => b.name === g.name && b.count > 0),
            )
            .map(({ name, Icon }) => (
              <button
                key={name}
                className={`nav-item ${query.group === name ? "active" : ""}`}
                data-group={name}
                onClick={() =>
                  navigate({
                    group: query.group === name ? "" : name,
                    extension: "",
                  })
                }
              >
                <Icon size={20} />
                <span>{name}</span>
                <small>
                  {summary.groups.find((g) => g.name === name)?.count || ""}
                </small>
                {query.group === name && <ChevronRight size={13} />}
              </button>
            ))}
        </section>
      </nav>
      <div className="sidebar-foot">
        <Check size={13} />
        仅索引指定文件夹
      </div>
    </aside>
  );
}
