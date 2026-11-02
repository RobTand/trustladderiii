import type { Card } from "./api/types.gen";

export function filterCards(cards: Card[], filter: { repo: string | null; query: string }): Card[] {
  void filter;
  return cards;
}
