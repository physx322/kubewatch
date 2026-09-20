// Table triable et filtrable (TanStack Table), en-tête collant.
import {
  flexRender,
  getCoreRowModel,
  getFilteredRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
} from "@tanstack/react-table";
import { CaretDown, CaretUp } from "@phosphor-icons/react";
import { useState, type ReactNode } from "react";

export function DataTable<T>({
  data,
  columns,
  filter = "",
  rowKey,
  selectedKey,
  onRowClick,
  onRowContextMenu,
  empty,
  initialSort,
}: {
  data: T[];
  columns: ColumnDef<T, unknown>[];
  filter?: string;
  rowKey: (row: T) => string;
  selectedKey?: string | null;
  onRowClick?: (row: T) => void;
  onRowContextMenu?: (row: T, e: React.MouseEvent) => void;
  empty?: ReactNode;
  initialSort?: SortingState;
}) {
  const [sorting, setSorting] = useState<SortingState>(initialSort ?? []);
  const table = useReactTable({
    data,
    columns,
    state: { sorting, globalFilter: filter },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    globalFilterFn: "includesString",
  });
  const rows = table.getRowModel().rows;
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          {table.getHeaderGroups().map((hg) => (
            <tr key={hg.id}>
              {hg.headers.map((h) => {
                const sorted = h.column.getIsSorted();
                return (
                  <th
                    key={h.id}
                    onClick={h.column.getToggleSortingHandler()}
                    style={{ width: h.getSize() !== 150 ? h.getSize() : undefined }}
                  >
                    <span className="row gap-4">
                      {flexRender(h.column.columnDef.header, h.getContext())}
                      {sorted === "asc" && <CaretUp size={11} />}
                      {sorted === "desc" && <CaretDown size={11} />}
                    </span>
                  </th>
                );
              })}
            </tr>
          ))}
        </thead>
        <tbody>
          {rows.map((r) => {
            const key = rowKey(r.original);
            return (
              <tr
                key={key}
                className={selectedKey === key ? "selected" : undefined}
                onClick={() => onRowClick?.(r.original)}
                onContextMenu={(e) => onRowContextMenu?.(r.original, e)}
              >
                {r.getVisibleCells().map((c) => (
                  <td key={c.id} className={(c.column.columnDef.meta as { num?: boolean } | undefined)?.num ? "num" : undefined}>
                    {flexRender(c.column.columnDef.cell, c.getContext())}
                  </td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>
      {rows.length === 0 && (empty ?? <div className="empty small">Aucun élément.</div>)}
    </div>
  );
}
