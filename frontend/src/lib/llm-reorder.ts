// Shared drag-and-drop reorder hook for LLM tables.
//
// Native HTML5 drag/drop is sufficient for a vertical list of ~dozens of
// rows; pulling in a D&D library would dwarf the feature. The hook tracks
// transient drag state and computes new priorities on drop using a
// 10-step grid (10, 20, 30 …) so subsequent inserts have headroom.

import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { AdminLlmRow } from "../api";
import { admin } from "../api";

interface ReorderState {
  draggingId: string | null;
  overId: string | null;
}

export interface DragRowProps {
  draggable: true;
  onDragStart: (e: React.DragEvent) => void;
  onDragOver: (e: React.DragEvent) => void;
  onDragLeave: () => void;
  onDrop: (e: React.DragEvent) => void;
  onDragEnd: () => void;
}

/** Drag visual state for one row. */
export interface RowDragState {
  isDragging: boolean;
  isOver: boolean;
}

export interface UseDragReorderResult {
  rowProps: (id: string) => DragRowProps;
  rowState: (id: string) => RowDragState;
  isPending: boolean;
}

/**
 * Returns drag handlers + visual state for each row, and persists the new
 * priority order via parallel PATCH calls when a drop lands. `sorted` must
 * be the currently displayed (priority-asc) list — its order is the
 * baseline we mutate against.
 */
export function useDragReorder(sorted: AdminLlmRow[]): UseDragReorderResult {
  const qc = useQueryClient();
  const [state, setState] = useState<ReorderState>({
    draggingId: null,
    overId: null,
  });

  const reorder = useMutation({
    mutationFn: async ({ from, to }: { from: string; to: string }) => {
      // Compute the post-drop visual order, then renumber every row in 10s.
      // We patch *every* row whose priority changes — usually that's just a
      // window around the moved row, but renumbering globally keeps the
      // grid clean and avoids arithmetic edge cases.
      const fromIdx = sorted.findIndex((r) => r.id === from);
      const toIdx = sorted.findIndex((r) => r.id === to);
      if (fromIdx < 0 || toIdx < 0 || fromIdx === toIdx) return;

      const next = [...sorted];
      const [moved] = next.splice(fromIdx, 1);
      next.splice(toIdx, 0, moved);

      const updates = next
        .map((row, i) => ({ row, newPri: (i + 1) * 10 }))
        .filter(({ row, newPri }) => (row.priority ?? 100) !== newPri);

      // Optimistically reorder the cache so the table snaps without a
      // round-trip. The server response will reconcile any drift.
      qc.setQueryData<{ items: AdminLlmRow[] } | undefined>(
        ["admin", "llm"],
        (prev) => {
          if (!prev) return prev;
          const map = new Map(updates.map((u) => [u.row.id, u.newPri]));
          return {
            ...prev,
            items: prev.items.map((row) =>
              map.has(row.id) ? { ...row, priority: map.get(row.id)! } : row,
            ),
          };
        },
      );

      await Promise.all(
        updates.map(({ row, newPri }) =>
          admin.llms.patch(row.id, { priority: newPri }),
        ),
      );
    },
    onSettled: () => qc.invalidateQueries({ queryKey: ["admin", "llm"] }),
  });

  const rowProps = (id: string): DragRowProps => ({
    draggable: true,
    onDragStart: (e) => {
      e.dataTransfer.effectAllowed = "move";
      // Firefox needs *some* data set or the drag won't initiate. The
      // value is ignored — we read state from React.
      e.dataTransfer.setData("text/plain", id);
      setState({ draggingId: id, overId: null });
    },
    onDragOver: (e) => {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      if (state.overId !== id) {
        setState((s) => ({ ...s, overId: id }));
      }
    },
    onDragLeave: () => {
      // Don't clear here — onDragOver on the next row arrives milliseconds
      // later and clobbering would cause a visual flicker.
    },
    onDrop: (e) => {
      e.preventDefault();
      const from = state.draggingId;
      setState({ draggingId: null, overId: null });
      if (from && from !== id) {
        reorder.mutate({ from, to: id });
      }
    },
    onDragEnd: () => {
      setState({ draggingId: null, overId: null });
    },
  });

  const rowState = (id: string): RowDragState => ({
    isDragging: state.draggingId === id,
    isOver: state.overId === id && state.draggingId !== null && state.draggingId !== id,
  });

  return { rowProps, rowState, isPending: reorder.isPending };
}

/** Grip icon shown in the drag handle column. */
export const DRAG_HANDLE_GLYPH = "⋮⋮";
