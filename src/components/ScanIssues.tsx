import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Copy } from "lucide-react";
import { command, errorText } from "../api";
import type { ScanIssuePage, Scope } from "../types";

const PAGE_SIZE = 50;
const kinds = { file: "文件", directory: "文件夹", path: "文件 / 文件夹" };

export function ScanIssues({ scope }: { scope: Scope }) {
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<ScanIssuePage | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    setPage(null);
    setError("");
    setCopied(false);
    command<ScanIssuePage>(
      "scan_issues",
      { id: scope.id, offset, limit: PAGE_SIZE },
      controller.signal,
    )
      .then((next) => {
        if (controller.signal.aborted) return;
        if (offset > 0 && offset >= next.total) {
          setOffset(
            Math.max(0, Math.ceil(next.total / PAGE_SIZE) - 1) * PAGE_SIZE,
          );
        } else {
          setPage(next);
        }
      })
      .catch((error) => {
        if (!controller.signal.aborted) setError(errorText(error));
      });
    return () => controller.abort();
  }, [
    scope.id,
    scope.lastScan,
    scope.freshness,
    scope.message,
    scope.scanIssueCount,
    offset,
  ]);

  return (
    <section
      className="scan-issues"
      aria-label={`${scope.name} 未完成的路径`}
      aria-busy={!page && !error}
    >
      <div className="scan-issues-heading">
        <strong>未完成的路径 · {page?.total ?? scope.scanIssueCount} 项</strong>
        {page && page.items.length > 0 && (
          <button
            className="text-button"
            onClick={async () => {
              try {
                await navigator.clipboard.writeText(
                  page.items.map((item) => item.path).join("\n"),
                );
                setCopied(true);
              } catch (error) {
                setError(errorText(error));
              }
            }}
          >
            <Copy size={13} />
            {copied ? "已复制" : "复制本页路径"}
          </button>
        )}
      </div>
      {error && (
        <p className="inline-error" role="alert">
          无法读取或复制路径：{error}
        </p>
      )}
      {!page && !error && <p role="status">正在读取未完成路径…</p>}
      {page && page.items.length === 0 && <p>这些路径已完成核对。</p>}
      {page && page.items.length > 0 && (
        <>
          <ul className="scan-issues-list">
            {page.items.map((item) => (
              <li key={item.path}>
                <div className="scan-issue-path">
                  <span>{kinds[item.kind]}</span>
                  <code>{item.path}</code>
                </div>
                <p>{item.reason}</p>
              </li>
            ))}
          </ul>
          {page.total > PAGE_SIZE && (
            <div className="scan-issues-pagination">
              <span>
                {page.offset + 1}–{page.offset + page.items.length} /{" "}
                {page.total} 项
              </span>
              <button
                className="icon-button"
                aria-label={`${scope.name} 未完成路径上一页`}
                disabled={offset === 0}
                onClick={() => setOffset(offset - PAGE_SIZE)}
              >
                <ChevronLeft size={16} />
              </button>
              <button
                className="icon-button"
                aria-label={`${scope.name} 未完成路径下一页`}
                disabled={offset + page.items.length >= page.total}
                onClick={() => setOffset(offset + PAGE_SIZE)}
              >
                <ChevronRight size={16} />
              </button>
            </div>
          )}
        </>
      )}
    </section>
  );
}
