import type { Board, Card, Column } from "./api/types.gen";

export const COLUMNS: Array<[Column, string]> = [
  ["Backlog", "Backlog"],
  ["Intake", "Intake"],
  ["Planning", "Planning"],
  ["Making", "Making"],
  ["Checking", "Checking"],
  ["Reviewing", "Reviewing"],
  ["Merging", "Merging"],
  ["NeedsRob", "Needs Rob"],
  ["Done", "Done"],
];

export function card(fields: Partial<Card> & { id: number }): Card {
  return {
    repo: "RobTand/tl3-sandbox",
    issue: 40 + fields.id,
    title: `Request ${fields.id}`,
    column: "Making",
    phase: "Making the change",
    ending: null,
    waiting_on: "the current task to finish",
    role: "Maker",
    model: "gpt-6.1-sol (high)",
    attempt: 1,
    round: 0,
    retries: { plan: 0, audit: 0, run_failures: 0, effect_failures: 0 },
    credit: 4,
    phase_since_ms: 1_000,
    last_progress: null,
    incidents: [],
    badges: [],
    ...fields,
  };
}

export function board(cards: Card[]): Board {
  return {
    revision: 7,
    built_at_ms: 1_000,
    flow: {
      columns: COLUMNS.map(([column, label]) => ({
        column,
        label,
        count: cards.filter((c) => c.column === column).length,
      })),
      in_flight: cards.filter((c) => c.column !== "Done").length,
      merged_last_hour: 0,
      merged_last_day: 0,
    },
    cards,
    unassigned_incidents: [],
  };
}
