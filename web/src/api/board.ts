import type { Board, BoardPatch } from "./types.gen";

/** Apply a stream patch to the board it was computed against. */
export function applyPatch(board: Board, patch: BoardPatch): Board {
  const cards = new Map(board.cards.map((card) => [card.id, card]));
  for (const id of patch.removals) cards.delete(id);
  for (const card of patch.upserts) cards.set(card.id, card);
  return {
    revision: patch.revision,
    built_at_ms: patch.built_at_ms,
    flow: patch.flow,
    cards: [...cards.values()].sort((a, b) => a.id - b.id),
    unassigned_incidents: patch.unassigned_incidents,
  };
}
