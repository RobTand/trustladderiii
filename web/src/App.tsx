import { useMemo, useState } from "react";
import { Select, ToggleGroup } from "radix-ui";
import { useLiveBoard } from "./api/live";
import { BoardColumn } from "./components/BoardColumn";
import { FlowStrip } from "./components/FlowStrip";
import { Status } from "./components/Status";
import { filterCards } from "./filter";
import { useTheme, type Theme } from "./theme";
import { useNow } from "./time";

const ALL_REPOS = "*";

function RepoFilter(props: { repos: string[]; value: string | null; onChange: (repo: string | null) => void }) {
  return (
    <Select.Root
      value={props.value ?? ALL_REPOS}
      onValueChange={(value) => props.onChange(value === ALL_REPOS ? null : value)}
    >
      <Select.Trigger
        aria-label="Repository"
        className="inline-flex h-8 min-w-36 items-center justify-between gap-2 rounded-md border border-border bg-surface px-2.5 text-sm text-text hover:border-border-strong"
      >
        <Select.Value />
        <Select.Icon className="text-faint">
          <svg aria-hidden="true" viewBox="0 0 12 12" className="size-3 fill-none stroke-current">
            <path d="M3 4.5 6 7.5 9 4.5" strokeWidth="1.4" />
          </svg>
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Content
          position="popper"
          sideOffset={4}
          className="z-10 min-w-[var(--radix-select-trigger-width)] rounded-md border border-border bg-surface p-1 text-sm shadow-lg"
        >
          <Select.Viewport>
            {[ALL_REPOS, ...props.repos].map((repo) => (
              <Select.Item
                key={repo}
                value={repo}
                className="cursor-default rounded px-2 py-1.5 text-text outline-none data-[highlighted]:bg-accent-soft"
              >
                <Select.ItemText>{repo === ALL_REPOS ? "All repositories" : repo}</Select.ItemText>
              </Select.Item>
            ))}
          </Select.Viewport>
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}

function ThemeSwitch() {
  const [theme, setTheme] = useTheme();
  return (
    <ToggleGroup.Root
      type="single"
      aria-label="Theme"
      value={theme}
      onValueChange={(value) => value && setTheme(value as Theme)}
      className="inline-flex h-8 rounded-md border border-border bg-surface p-0.5"
    >
      {(["system", "light", "dark"] as const).map((option) => (
        <ToggleGroup.Item
          key={option}
          value={option}
          className="rounded px-2 text-xs text-muted capitalize data-[state=on]:bg-surface-2 data-[state=on]:text-text"
        >
          {option}
        </ToggleGroup.Item>
      ))}
    </ToggleGroup.Root>
  );
}

export function App() {
  const { board, connection } = useLiveBoard();
  const now = useNow();
  const [repo, setRepo] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const repos = useMemo(() => [...new Set(board?.cards.map((card) => card.repo))].sort(), [board]);
  const cards = useMemo(() => (board ? filterCards(board.cards, { repo, query }) : []), [board, repo, query]);
  return (
    <div className="flex h-full flex-col gap-3 p-3 sm:p-4">
      <header className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <h1 className="flex items-baseline gap-2 text-base font-semibold">
          TL3 <span className="font-normal text-muted">console</span>
        </h1>
        <Status connection={connection} builtAt={board?.built_at_ms ?? null} />
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <RepoFilter repos={repos} value={repo} onChange={setRepo} />
          <input
            type="search"
            aria-label="Search by issue or title"
            placeholder="Issue # or title"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="h-8 w-48 rounded-md border border-border bg-surface px-2.5 text-sm text-text placeholder:text-faint hover:border-border-strong"
          />
          <ThemeSwitch />
        </div>
      </header>
      {board ? (
        <>
          <FlowStrip flow={board.flow} />
          {board.unassigned_incidents.length > 0 && (
            <section
              aria-label="Open incidents without a request"
              className="rounded-md border border-danger/30 bg-danger-soft px-3 py-2 text-xs text-danger"
            >
              <ul className="flex flex-col gap-0.5">
                {board.unassigned_incidents.map((incident) => (
                  <li key={incident.id}>
                    <span className="font-mono">{incident.id}</span> {incident.summary}
                  </li>
                ))}
              </ul>
            </section>
          )}
          <main className="flex min-h-0 flex-1 snap-x gap-3 overflow-x-auto pb-1">
            {board.flow.columns.map((column) => {
              // The board keeps a day of endings; the count above covers them all.
              const older = column.count - board.cards.filter((card) => card.column === column.column).length;
              return (
                <BoardColumn
                  key={column.column}
                  column={column.column}
                  label={column.label}
                  now={now}
                  cards={cards.filter((card) => card.column === column.column)}
                  {...(older > 0 ? { note: `${older} older ${older === 1 ? "ending" : "endings"} not shown` } : {})}
                />
              );
            })}
          </main>
        </>
      ) : (
        <main className="flex flex-1 items-center justify-center text-sm text-muted">
          Waiting for the console's first snapshot…
        </main>
      )}
    </div>
  );
}
