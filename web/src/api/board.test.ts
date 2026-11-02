import { applyPatch } from "./board";
import type { Board } from "./types.gen";
import { board, card } from "../fixtures";

describe("applyPatch", () => {
  it("replaces changed cards, adds new ones, drops removals and keeps id order", () => {
    const before: Board = board([card({ id: 1 }), card({ id: 2 }), card({ id: 4 })]);
    const after = applyPatch(before, {
      revision: 9,
      built_at_ms: 2_000,
      flow: { ...before.flow, in_flight: 3 },
      upserts: [card({ id: 3, title: "New" }), card({ id: 1, column: "Checking" })],
      removals: [2],
      unassigned_incidents: [{ id: "inc-1", summary: "Hindsight is down" }],
    });
    expect(after.cards.map((c) => c.id)).toEqual([1, 3, 4]);
    expect(after.cards[0]?.column).toBe("Checking");
    expect(after.cards[1]?.title).toBe("New");
    expect(after.revision).toBe(9);
    expect(after.built_at_ms).toBe(2_000);
    expect(after.flow.in_flight).toBe(3);
    expect(after.unassigned_incidents).toHaveLength(1);
    expect(before.cards.map((c) => c.id)).toEqual([1, 2, 4]);
  });
});
