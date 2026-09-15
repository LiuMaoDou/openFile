import { useEffect, useState } from "react";
import { BookOpen, ChevronDown } from "lucide-react";
import { command, errorText } from "../api";
import { formatDate } from "./FileIcon";
interface FolderStatus {
  id: string;
  name: string;
  enabled: boolean;
  paused: boolean;
  online: boolean;
  eligible: number;
  ready: number;
  pending: number;
  failed: number;
  skipped: number;
  truncated: number;
  unsupported: number;
  lastIndexed: number | null;
}
interface Status {
  scopes: FolderStatus[];
  issues: { name: string; message: string }[];
}
export function ContentIndex({
  scopeId,
  changed,
  fail,
}: {
  scopeId: string;
  changed: () => void;
  fail: (text: string) => void;
}) {
  const [data, setData] = useState<Status | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const controller = new AbortController();
    setData(null);
    async function poll() {
      try {
        const next = await command<Status>(
          "content_status",
          { scopeId },
          controller.signal,
        );
        if (!stopped) setData(next);
      } catch (e) {
        if (!stopped) fail(errorText(e));
      }
      if (!stopped) timer = setTimeout(poll, 1800);
    }
    void poll();
    return () => {
      stopped = true;
      controller.abort();
      clearTimeout(timer);
    };
  }, [scopeId, revision, fail]);
  const enabled = data?.scopes.filter((s) => s.enabled) ?? [];
  const ready = enabled.reduce((n, s) => n + s.ready, 0);
  const pending = enabled.reduce((n, s) => n + s.pending, 0);
  const statusText = !data
    ? "读取状态…"
    : !enabled.length
      ? "未启用 · 可按文件夹开启"
      : enabled.length > 1
        ? `已启用 ${enabled.length} 个文件夹 · 展开查看各文件夹进度`
        : `${ready.toLocaleString()} 份文档可搜索${pending ? ` · ${pending.toLocaleString()} 份待处理` : " · 本轮处理完成"}`;
  async function act(id: string, action: string) {
    setBusy(true);
    try {
      await command("content_action", { id, action });
      setRevision((v) => v + 1);
      changed();
    } catch (e) {
      fail(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="content-index" aria-label="内容索引">
      <button
        className="content-index-toggle"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <BookOpen size={16} />
        <strong>内容索引</strong>
        <span title={statusText}>{statusText}</span>
        <ChevronDown size={15} />
      </button>
      {open && (
        <div className="content-index-details">
          <p>
            文字仅保存在本机。支持文本、代码、带文字层的
            PDF、DOCX、XLSX、PPTX；扫描图片暂不支持 OCR。单文件上限 64
            MiB，最多提取 2 MiB 文字。
          </p>
          {data?.scopes.map((s) => (
            <div className="content-folder" key={s.id}>
              <div>
                <strong>{s.name}</strong>
                <small>
                  {!s.enabled
                    ? `${s.eligible} 份文档可建立内容索引`
                    : `${s.paused ? "已暂停 · " : !s.online ? "等待文件夹上线 · " : ""}完成 ${s.ready}/${s.eligible} · 失败 ${s.failed} · 跳过 ${s.skipped}${s.truncated ? ` · 部分索引 ${s.truncated}` : ""}`}
                </small>
                <small>
                  {s.unsupported} 个其他格式文件不读取内容
                  {s.lastIndexed && s.enabled
                    ? ` · 最近处理 ${formatDate(s.lastIndexed)}`
                    : ""}
                </small>
              </div>
              <div className="content-folder-actions">
                {!s.enabled ? (
                  <button
                    className="button outline"
                    disabled={busy}
                    onClick={() => void act(s.id, "enable")}
                  >
                    开启内容索引
                  </button>
                ) : (
                  <>
                    <button
                      className="button"
                      disabled={busy}
                      onClick={() =>
                        void act(s.id, s.paused ? "resume" : "pause")
                      }
                    >
                      {s.paused ? "继续" : "暂停"}
                    </button>
                    <button
                      className="button"
                      disabled={busy}
                      onClick={() => void act(s.id, "rebuild")}
                    >
                      重建
                    </button>
                    <button
                      className="button"
                      disabled={busy}
                      onClick={() => void act(s.id, "disable")}
                    >
                      关闭索引
                    </button>
                  </>
                )}
              </div>
            </div>
          ))}
          {data && data.issues.length > 0 && (
            <details className="content-issues">
              <summary>处理说明（最近 {data.issues.length} 项）</summary>
              {data.issues.map((e, i) => (
                <p key={`${i}-${e.name}`}>
                  <strong>{e.name}</strong>：{e.message}
                </p>
              ))}
            </details>
          )}
          <small>
            关闭会清理仅属于该文件夹的内容索引；重建会重新提取文字。索引期间可以继续搜索已完成的文档。
          </small>
        </div>
      )}
    </section>
  );
}
