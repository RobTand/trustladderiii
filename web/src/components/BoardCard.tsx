import type { Badge, Card, Ending } from "../api/types.gen";
import { elapsed } from "../time";

const BADGES: Record<Badge, { label: string; tone: string; hint: string }> = {
  Paused: { label: "Paused", tone: "bg-warn-soft text-warn", hint: "Paused by a command; resume to continue" },
  Incident: { label: "Incident", tone: "bg-danger-soft text-danger", hint: "An open incident blocks this request" },
  MakerBlocked: {
    label: "Maker blocked",
    tone: "bg-danger-soft text-danger",
    hint: "The maker reported that the environment stopped it",
  },
  NeedsRob: { label: "Needs Rob", tone: "bg-info-soft text-info", hint: "Waiting on an answer or decision from Rob" },
  WaitingForCapacity: {
    label: "Waiting for capacity",
    tone: "bg-surface-2 text-muted",
    hint: "Waiting for a planner, a slot or its files",
  },
};

const ENDINGS: Record<Ending, { label: string; tone: string; hint: string }> = {
  Merged: { label: "Merged", tone: "bg-ok-soft text-ok", hint: "Merged into the default branch" },
  Stopped: { label: "Stopped", tone: "bg-surface-2 text-muted", hint: "Stopped by a command" },
  Replaced: { label: "Replaced", tone: "bg-surface-2 text-muted", hint: "A newer request replaced this one" },
  Duplicate: { label: "Duplicate", tone: "bg-surface-2 text-muted", hint: "Another request covers this work" },
  AlreadyDone: { label: "Already done", tone: "bg-surface-2 text-muted", hint: "The work was already done" },
  NotActionable: { label: "Not actionable", tone: "bg-surface-2 text-muted", hint: "The request can't be acted on" },
};

function Chip(props: { tone: string; title: string; children: string }) {
  return (
    <span
      title={props.title}
      className={`inline-flex h-5 items-center rounded px-1.5 text-2xs font-medium whitespace-nowrap ${props.tone}`}
    >
      {props.children}
    </span>
  );
}

export function BoardCard({ card, now }: { card: Card; now: number }) {
  const [owner, name] = card.repo.split("/");
  const retries =
    card.retries.plan + card.retries.audit + card.retries.run_failures + card.retries.effect_failures;
  const worker = [card.role, card.model].filter((part): part is string => part !== null).join(" · ");
  const chips = [
    ...(card.ending === null ? [] : [ENDINGS[card.ending]]),
    ...card.badges.map((badge) => BADGES[badge]),
  ];
  return (
    <article
      aria-label={`${card.repo}#${card.issue}: ${card.title}`}
      data-card={card.id}
      tabIndex={0}
      className="group rounded-md border border-border bg-surface p-2.5 shadow-[var(--shadow)] transition-colors hover:border-border-strong focus-visible:border-accent"
    >
      <header className="flex items-center gap-1.5 text-2xs text-muted">
        <span className="truncate" title={card.repo}>
          {name ?? owner}
        </span>
        <span className="font-mono text-faint">#{card.issue}</span>
        <span className="ml-auto font-mono text-faint" aria-label="Time in stage">
          {card.phase_since_ms === null ? "–" : elapsed(now - card.phase_since_ms)}
        </span>
      </header>
      <h3 className="mt-1 line-clamp-2 text-sm font-medium text-text">{card.title}</h3>
      <p className="mt-1.5 text-xs text-text">{card.phase}</p>
      {card.waiting_on !== "none" && (
        <p className="truncate text-xs text-muted" title={`Waiting on ${card.waiting_on}`}>
          <span className="text-faint">Waiting on </span>
          {card.waiting_on}
        </p>
      )}
      {worker && <p className="mt-1 truncate text-xs text-muted">{worker}</p>}
      <footer className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-2xs text-muted">
        {card.attempt > 0 && (
          <span>{card.round === 0 ? `Attempt ${card.attempt}` : `Attempt ${card.attempt} · Round ${card.round}`}</span>
        )}
        {retries > 0 && <span>{retries === 1 ? "1 retry" : `${retries} retries`}</span>}
        <span className="ml-auto inline-flex items-center gap-1" title="Credit left">
          <svg aria-hidden="true" viewBox="0 0 12 12" className="size-3 fill-none stroke-current">
            <circle cx="6" cy="6" r="4.5" strokeWidth="1.2" />
            <circle cx="6" cy="6" r="1.5" strokeWidth="1.2" />
          </svg>
          <span
            aria-label="Credit"
            className={card.credit !== null && card.credit <= 0 ? "font-medium text-danger" : "text-text"}
          >
            {card.credit ?? "–"}
          </span>
        </span>
      </footer>
      {chips.length > 0 && (
        <ul className="mt-2 flex flex-wrap gap-1" aria-label="Badges">
          {chips.map((chip) => (
            <li key={chip.label}>
              <Chip tone={chip.tone} title={chip.hint}>
                {chip.label}
              </Chip>
            </li>
          ))}
        </ul>
      )}
    </article>
  );
}
