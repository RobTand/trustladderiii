import type { Board, BoardPatch } from "./types.gen";

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

export class BoardStream {
  constructor(handlers: StreamHandlers, environment: StreamEnvironment) {
    void handlers;
    void environment;
  }

  start(): void {}

  stop(): void {}
}
