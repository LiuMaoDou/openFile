import test from "node:test";
import assert from "node:assert/strict";
import {
  FILE_COLUMNS,
  defaultFileColumnLayout,
  fileColumnGridTemplate,
  fileColumnMinTableWidth,
  moveFileColumn,
  parseFileColumnLayout,
  setFileColumnWidth,
} from "../src/components/fileTableColumns.ts";

test("new and malformed saved layouts restore safe defaults", () => {
  const expected = FILE_COLUMNS.map((column) => column.key);
  assert.deepEqual(defaultFileColumnLayout(), { order: expected, widths: {} });
  for (const saved of [null, "", "not JSON", "null", "[]"]) {
    assert.deepEqual(parseFileColumnLayout(saved), {
      order: expected,
      widths: {},
    });
  }
});

test("saved order preserves known keys, appends missing ones, and rejects duplicates", () => {
  const layout = parseFileColumnLayout(
    JSON.stringify({
      order: ["mtime", "name", "name", "unknown"],
      widths: { name: 1, directory: 250.6, mtime: 9000, size: "200" },
    }),
  );
  assert.deepEqual(layout.order, [
    "mtime", "name", "directory", "size", "extension",
  ]);
  assert.deepEqual(layout.widths, {
    name: 180,
    directory: 251,
    mtime: 1400,
  });
});

test("moving columns before and after keeps all columns and preserves widths", () => {
  const initial = setFileColumnWidth(defaultFileColumnLayout(), "name", 360);
  const moved = moveFileColumn(initial, "name", "mtime", "after");
  assert.deepEqual(moved.order, [
    "directory", "size", "mtime", "name", "extension",
  ]);
  assert.deepEqual(moved.widths, { name: 360 });
  const restored = moveFileColumn(moved, "name", "directory", "before");
  assert.deepEqual(restored.order, initial.order);
  assert.equal(moveFileColumn(restored, "name", "directory", "before"), restored);
  assert.equal(moveFileColumn(restored, "size", "size", "after"), restored);
});

test("widths are clamped and reflected in the template and minimum table width", () => {
  const initial = defaultFileColumnLayout();
  assert.equal(fileColumnMinTableWidth(initial), 908);
  assert.equal(
    fileColumnGridTemplate(initial),
    "32px var(--file-column-name) var(--file-column-directory) " +
      "var(--file-column-size) var(--file-column-mtime) " +
      "var(--file-column-extension) 76px",
  );
  const resized = setFileColumnWidth(initial, "directory", 480);
  assert.equal(resized.widths.directory, 480);
  assert.match(fileColumnGridTemplate(resized), /480px/);
  assert.equal(fileColumnMinTableWidth(resized), 1208);
  assert.equal(setFileColumnWidth(resized, "directory", 480), resized);
  assert.equal(setFileColumnWidth(resized, "size", -20).widths.size, 72);
  assert.equal(setFileColumnWidth(resized, "name", Infinity), resized);
});
