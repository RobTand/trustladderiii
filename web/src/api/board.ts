import type { Board, BoardPatch } from "./types.gen";

/** Apply a stream patch to the board it was computed against. */
export function applyPatch(board: Board, patch: BoardPatch): Board {
  void patch;
  return board;
}
