export const FILE_COLUMNS = [
  { key: "name", label: "文件名", minWidth: 180, defaultWidth: 240 },
  { key: "directory", label: "所在目录", minWidth: 180, defaultWidth: 260 },
  { key: "size", label: "大小", minWidth: 72, defaultWidth: 86 },
  { key: "mtime", label: "修改时间", minWidth: 116, defaultWidth: 154 },
  { key: "extension", label: "类型", minWidth: 64, defaultWidth: 72 },
] as const;

export type FileColumnKey = (typeof FILE_COLUMNS)[number]["key"];
export interface FileColumnLayout {
  order: FileColumnKey[];
  widths: Partial<Record<FileColumnKey, number>>;
}

export const FILE_COLUMN_STORAGE_KEY = "filem-file-columns-v1";
export const MAX_FILE_COLUMN_WIDTH = 1400;
const columnKeys: readonly string[] = FILE_COLUMNS.map((column) => column.key);

export function defaultFileColumnLayout(): FileColumnLayout {
  return { order: FILE_COLUMNS.map((column) => column.key), widths: {} };
}

export function parseFileColumnLayout(raw: string | null): FileColumnLayout {
  if (!raw) return defaultFileColumnLayout();
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object") return defaultFileColumnLayout();
    const saved = value as { order?: unknown; widths?: unknown };
    const savedOrder = Array.isArray(saved.order)
      ? saved.order.filter(
          (key): key is FileColumnKey =>
            typeof key === "string" && columnKeys.includes(key),
        )
      : [];
    const order = [...new Set(savedOrder)];
    for (const column of FILE_COLUMNS) {
      if (!order.includes(column.key)) order.push(column.key);
    }
    const savedWidths =
      saved.widths && typeof saved.widths === "object"
        ? (saved.widths as Record<string, unknown>)
        : {};
    const widths: FileColumnLayout["widths"] = {};
    for (const column of FILE_COLUMNS) {
      const width = savedWidths[column.key];
      if (typeof width === "number" && Number.isFinite(width)) {
        widths[column.key] = Math.min(
          MAX_FILE_COLUMN_WIDTH,
          Math.max(column.minWidth, Math.round(width)),
        );
      }
    }
    return { order, widths };
  } catch {
    return defaultFileColumnLayout();
  }
}

export function moveFileColumn(
  layout: FileColumnLayout,
  source: FileColumnKey,
  target: FileColumnKey,
  side: "before" | "after",
): FileColumnLayout {
  if (
    source === target ||
    !layout.order.includes(source) ||
    !layout.order.includes(target)
  ) {
    return layout;
  }
  const order = layout.order.filter((key) => key !== source);
  const targetIndex = order.indexOf(target);
  order.splice(targetIndex + (side === "after" ? 1 : 0), 0, source);
  return order.every((key, index) => key === layout.order[index])
    ? layout
    : { ...layout, order };
}

export function setFileColumnWidth(
  layout: FileColumnLayout,
  key: FileColumnKey,
  requestedWidth: number,
): FileColumnLayout {
  const column = FILE_COLUMNS.find((candidate) => candidate.key === key);
  if (!column || !Number.isFinite(requestedWidth)) return layout;
  const width = Math.min(
    MAX_FILE_COLUMN_WIDTH,
    Math.max(column.minWidth, Math.round(requestedWidth)),
  );
  return layout.widths[key] === width
    ? layout
    : { ...layout, widths: { ...layout.widths, [key]: width } };
}

export function fileColumnGridTemplate(layout: FileColumnLayout): string {
  return [
    "32px",
    ...layout.order.map((key) =>
      layout.widths[key] === undefined
        ? `var(--file-column-${key})`
        : `${layout.widths[key]}px`,
    ),
    "76px",
  ].join(" ");
}

export function fileColumnMinTableWidth(layout: FileColumnLayout): number {
  // Selection + actions + six gaps + horizontal padding.
  const chrome = 32 + 76 + 6 * 12 + 2 * 28;
  return Math.max(
    870,
    chrome +
      FILE_COLUMNS.reduce(
        (sum, column) =>
          sum +
          (layout.widths[column.key] ??
            (column.key === "name" || column.key === "directory"
              ? column.minWidth
              : column.defaultWidth)),
        0,
      ),
  );
}
