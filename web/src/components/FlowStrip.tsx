import type { Flow } from "../api/types.gen";

function Stat(props: { label: string; value: number; strong?: boolean }) {
  return (
    <div className="flex min-w-[5.5rem] flex-col gap-0.5 px-3 py-2">
      <dt className="text-2xs whitespace-nowrap text-muted">{props.label}</dt>
      <dd className={`font-mono text-base ${props.strong ? "font-semibold text-text" : "text-text"}`}>
        {props.value}
      </dd>
    </div>
  );
}

/** Counts per stage, then the factory's totals. */
export function FlowStrip({ flow }: { flow: Flow }) {
  return (
    <dl
      aria-label="Flow"
      className="flex shrink-0 overflow-x-auto rounded-lg border border-border bg-surface shadow-[var(--shadow)]"
    >
      <div className="flex divide-x divide-border">
        {flow.columns.map((count) => (
          <Stat key={count.column} label={count.label} value={count.count} />
        ))}
      </div>
      <div className="ml-auto flex divide-x divide-border border-l border-border bg-surface-2/50">
        <Stat label="In flight" value={flow.in_flight} strong />
        <Stat label="Merged, last hour" value={flow.merged_last_hour} strong />
        <Stat label="Merged, last day" value={flow.merged_last_day} strong />
      </div>
    </dl>
  );
}
