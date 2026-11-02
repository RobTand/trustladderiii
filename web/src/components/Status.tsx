import type { Connection } from "../api/live";

const CONNECTION: Record<Connection, { label: string; dot: string }> = {
  connecting: { label: "Connecting", dot: "bg-faint" },
  live: { label: "Live", dot: "bg-ok" },
  reconnecting: { label: "Reconnecting", dot: "bg-warn" },
  "database-offline": { label: "Database offline", dot: "bg-danger" },
};

function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/** The connection state is a live region; the update time beside it is not, so it isn't read out on every change. */
export function Status({ connection, builtAt }: { connection: Connection; builtAt: number | null }) {
  const state = CONNECTION[connection];
  return (
    <div className="flex items-center gap-2 text-xs text-muted">
      <span
        role="status"
        className="inline-flex items-center gap-1.5 rounded-full border border-border bg-surface px-2 py-0.5 text-text"
      >
        <span aria-hidden="true" className={`size-1.5 rounded-full ${state.dot}`} />
        {state.label}
      </span>
      {builtAt !== null && (
        <span className="hidden sm:inline">
          Updated <time dateTime={new Date(builtAt).toISOString()}>{clock(builtAt)}</time>
        </span>
      )}
    </div>
  );
}
