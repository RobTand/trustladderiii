import { useId } from "react";
import type { Card, Column } from "../api/types.gen";
import { BoardCard } from "./BoardCard";

/** Done shows the newest endings first; every other column keeps request order. */
function ordered(column: Column, cards: Card[]): Card[] {
  return column === "Done"
    ? [...cards].sort((a, b) => (b.phase_since_ms ?? 0) - (a.phase_since_ms ?? 0) || b.id - a.id)
    : [...cards].sort((a, b) => a.id - b.id);
}

export function BoardColumn(props: { column: Column; label: string; cards: Card[]; now: number }) {
  const heading = useId();
  return (
    <section
      aria-labelledby={heading}
      className="flex max-h-full w-[85vw] shrink-0 snap-start flex-col rounded-lg bg-surface-2/60 sm:w-72"
    >
      <h2 className="flex items-center gap-2 px-3 pt-2.5 pb-2 text-xs font-semibold text-muted">
        <span id={heading} className="tracking-wide uppercase">
          {props.label}
        </span>
        <span className="rounded bg-surface px-1.5 font-mono text-2xs font-medium text-faint">
          {props.cards.length}
        </span>
      </h2>
      <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-2 pb-2">
        {props.cards.length === 0 ? (
          <p className="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-faint">
            Nothing here
          </p>
        ) : (
          ordered(props.column, props.cards).map((card) => <BoardCard key={card.id} card={card} now={props.now} />)
        )}
      </div>
    </section>
  );
}
