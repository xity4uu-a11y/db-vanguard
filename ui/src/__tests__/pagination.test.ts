import { describe, expect, it } from "vitest";
import { paginateRows, sortRows } from "../lib/sqlEngine";
import type { CellValue } from "../lib/types";

describe("Data Grid Result Pagination and Type-Aware Sorting", () => {
  it("slices pages accurately and clamps out-of-range page numbers", () => {
    const rows = Array.from({ length: 105 }, (_, i) => [i + 1]);

    const first = paginateRows(rows, 1, 50);
    expect(first.page).toBe(1);
    expect(first.totalPages).toBe(3);
    expect(first.items).toHaveLength(50);
    expect(first.startIndex).toBe(0);
    expect(first.endIndex).toBe(50);

    const last = paginateRows(rows, 99, 50);
    expect(last.page).toBe(3);
    expect(last.items).toHaveLength(5);
    expect(last.startIndex).toBe(100);
    expect(last.endIndex).toBe(105);

    const under = paginateRows(rows, 0, 25);
    expect(under.page).toBe(1);
    expect(under.totalPages).toBe(5);

    const empty = paginateRows([], 1, 50);
    expect(empty.totalItems).toBe(0);
    expect(empty.totalPages).toBe(1);
    expect(empty.items).toHaveLength(0);
  });

  it("sorts numeric, text, and NULL values deterministically without mutating input", () => {
    const original: CellValue[][] = [
      [10, "delta"],
      [2, "alpha"],
      [null, "omega"],
      [100, "beta"],
    ];

    const asc = sortRows(original, 0, "asc");
    expect(asc.map((r) => r[0])).toEqual([2, 10, 100, null]);

    const desc = sortRows(original, 0, "desc");
    expect(desc.map((r) => r[0])).toEqual([null, 100, 10, 2]);

    // Original array remains untouched
    expect(original[0][0]).toBe(10);
  });
});
