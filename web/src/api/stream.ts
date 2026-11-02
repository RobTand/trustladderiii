import type { Board, BoardPatch, PatchEvent } from "./types.gen";

export type StreamState = "connecting" | "open" | "reconnecting";

export interface StreamHandlers {
  snapshot(board: Board): void;
  patch(patch: BoardPatch): void;
  database(connected: boolean): void;
  state(state: StreamState): void;
}

/** The parts of EventSource the stream uses. */
export interface Source {
  readonly readyState: number;
  onopen: ((event: Event) => void) | null;
  onerror: ((event: Event) => void) | null;
  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void;
  close(): void;
}

/** What the stream needs from the browser, so tests can stand in for it. */
export interface StreamEnvironment {
  open(url: string): Source;
  /** Run once after a delay; returns a cancel function. */
  later(ms: number, run: () => void): () => void;
  hidden(): boolean;
  /** Call on each visibility change; returns an unsubscribe function. */
  onVisibility(run: () => void): () => void;
}

export const CLOSED = 2;
const FIRST_RETRY_MS = 1_000;
const MAX_RETRY_MS = 30_000;

export function browserEnvironment(): StreamEnvironment {
  return {
    open: (url) => new EventSource(url),
    later: (ms, run) => {
      const timer = window.setTimeout(run, ms);
      return () => window.clearTimeout(timer);
    },
    hidden: () => document.hidden,
    onVisibility: (run) => {
      document.addEventListener("visibilitychange", run);
      return () => document.removeEventListener("visibilitychange", run);
    },
  };
}

/**
 * The console's event stream. Each connection starts with a snapshot, and
 * each patch names the board it applies to; a patch for another board means
 * events were missed, so the stream reconnects for a fresh snapshot. The
 * browser retries a dropped stream by itself, but not one that got a non-200
 * answer (a tunnel's 502, say); that one reconnects here after a bounded
 * backoff. A hidden tab closes its stream so many tabs don't hold every
 * HTTP/1.1 connection, and reconnects when shown.
 */
export class BoardStream {
  private source: Source | null = null;
  private shown: string | null = null;
  private opened = false;
  private retryMs = FIRST_RETRY_MS;
  private cancelRetry: (() => void) | null = null;
  private stopVisibility: (() => void) | null = null;

  constructor(
    private readonly handlers: StreamHandlers,
    private readonly environment: StreamEnvironment,
  ) {}

  start(): void {
    this.stopVisibility = this.environment.onVisibility(() => {
      if (this.environment.hidden()) this.disconnect();
      else if (!this.source) this.connect();
    });
    if (!this.environment.hidden()) this.connect();
  }

  stop(): void {
    this.stopVisibility?.();
    this.stopVisibility = null;
    this.disconnect();
  }

  private connect(): void {
    this.disconnect();
    this.handlers.state(this.opened ? "reconnecting" : "connecting");
    const source = this.environment.open("/api/stream");
    this.source = source;
    source.onopen = () => {
      this.retryMs = FIRST_RETRY_MS;
      this.opened = true;
      this.handlers.state("open");
    };
    source.onerror = () => {
      if (this.source !== source) return;
      this.handlers.state("reconnecting");
      if (source.readyState === CLOSED) this.retry();
    };
    source.addEventListener("snapshot", (event) => {
      this.shown = event.lastEventId;
      this.handlers.snapshot(JSON.parse(event.data) as Board);
    });
    source.addEventListener("patch", (event) => {
      const { base, patch } = JSON.parse(event.data) as PatchEvent;
      if (base !== this.shown) {
        this.connect();
        return;
      }
      this.shown = event.lastEventId;
      this.handlers.patch(patch);
    });
    source.addEventListener("database", (event) => {
      this.handlers.database((JSON.parse(event.data) as { connected: boolean }).connected);
    });
  }

  private retry(): void {
    this.source?.close();
    this.source = null;
    const delay = this.retryMs;
    this.retryMs = Math.min(this.retryMs * 2, MAX_RETRY_MS);
    this.cancelRetry = this.environment.later(delay, () => {
      this.cancelRetry = null;
      this.connect();
    });
  }

  private disconnect(): void {
    this.cancelRetry?.();
    this.cancelRetry = null;
    this.source?.close();
    this.source = null;
    this.shown = null;
  }
}
