import type { Card } from "./api/types.gen";

export interface CardFilter {
  /** `owner/name`, or null for every repo. */
  repo: string | null;
  /** An issue number (with or without `#`), or words from the title. */
  query: string;
}

export function filterCards(cards: Card[], filter: CardFilter): Card[] {
  const query = filter.query.trim().toLowerCase();
  const issue = /^#?(\d+)$/.exec(query)?.[1];
  const words = query.split(/\s+/).filter(Boolean);
  return cards.filter((card) => {
    if (filter.repo !== null && card.repo !== filter.repo) return false;
    if (issue !== undefined) return String(card.issue) === issue;
    const title = card.title.toLowerCase();
    return words.every((word) => title.includes(word));
  });
}
