import { BoardStream, CLOSED, type Source, type StreamEnvironment, type StreamHandlers } from "./stream";
import { board, card } from "../fixtures";
import type { BoardPatch } from "./types.gen";

class FakeSource implements Source {
  readyState = 0;
  closed = false;
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  listeners = new Map<string, (event: MessageEvent<string>) => void>();
  addEventListener(type: string, listener: (event: MessageEvent<string>) => void) {
    this.listeners.set(type, listener);
  }
  close() {
    this.closed = true;
    this.readyState = CLOSED;
  }
  open() {
    this.readyState = 1;
    this.onopen?.(new Event("open"));
  }
  emit(type: string, id: string, data: unknown) {
    this.listeners.get(type)?.(new MessageEvent(type, { data: JSON.stringify(data), lastEventId: id }));
  }
  fail(closed: boolean) {
    this.readyState = closed ? CLOSED : 0;
    this.onerror?.(new Event("error"));
  }
}

function setup(hidden = false) {
  const sources: FakeSource[] = [];
  const timers: Array<{ ms: number; run: () => void; cancelled: boolean }> = [];
  let visibility: (() => void) | null = null;
  const state = { hidden };
  const environment: StreamEnvironment = {
    open: () => {
      const source = new FakeSource();
      sources.push(source);
      return source;
    },
    later: (ms, run) => {
      const timer = { ms, run, cancelled: false };
      timers.push(timer);
      return () => (timer.cancelled = true);
    },
    hidden: () => state.hidden,
    onVisibility: (run) => {
      visibility = run;
      return () => (visibility = null);
    },
  };
  const handlers = {
    snapshot: vi.fn<StreamHandlers["snapshot"]>(),
    patch: vi.fn<StreamHandlers["patch"]>(),
    database: vi.fn<StreamHandlers["database"]>(),
    state: vi.fn<StreamHandlers["state"]>(),
  };
  const stream = new BoardStream(handlers, environment);
  const setHidden = (value: boolean) => {
    state.hidden = value;
    visibility?.();
  };
  const fire = () => {
    const timer = timers.find((t) => !t.cancelled);
    expect(timer).toBeDefined();
    timer!.cancelled = true;
    timer!.run();
    return timer!.ms;
  };
  return { stream, sources, timers, handlers, setHidden, fire };
}

const patch = (revision: number): BoardPatch => ({
  revision,
  built_at_ms: 1,
  flow: board([]).flow,
  upserts: [card({ id: 1 })],
  removals: [],
  unassigned_incidents: [],
});

describe("BoardStream", () => {
  it("applies patches to the board they name and resnapshots on a gap", () => {
    const { stream, sources, handlers } = setup();
    stream.start();
    expect(sources).toHaveLength(1);
    const first = sources[0]!;
    first.open();
    first.emit("snapshot", "7.1", board([]));
    first.emit("patch", "8.2", { base: "7.1", patch: patch(8) });
    expect(handlers.snapshot).toHaveBeenCalledTimes(1);
    expect(handlers.patch).toHaveBeenCalledWith(patch(8));
    first.emit("patch", "9.4", { base: "8.3", patch: patch(9) });
    expect(handlers.patch).toHaveBeenCalledTimes(1);
    expect(first.closed).toBe(true);
    expect(sources).toHaveLength(2);
  });

  it("reconnects a closed stream with a bounded backoff that an open resets", () => {
    const { stream, sources, fire, handlers } = setup();
    stream.start();
    const delays: number[] = [];
    for (let attempt = 0; attempt < 7; attempt++) {
      sources.at(-1)!.fail(true);
      expect(handlers.state).toHaveBeenLastCalledWith("reconnecting");
      const opened = sources.length;
      delays.push(fire());
      expect(sources).toHaveLength(opened + 1);
    }
    expect(delays).toEqual([1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]);
    sources.at(-1)!.open();
    expect(handlers.state).toHaveBeenLastCalledWith("open");
    sources.at(-1)!.fail(true);
    expect(fire()).toBe(1_000);
  });

  it("leaves a stream the browser is already retrying alone", () => {
    const { stream, sources, timers } = setup();
    stream.start();
    sources[0]!.fail(false);
    expect(timers).toHaveLength(0);
    expect(sources).toHaveLength(1);
  });

  it("closes while hidden and reopens with a fresh snapshot when visible", () => {
    const { stream, sources, setHidden, handlers, timers } = setup();
    stream.start();
    sources[0]!.open();
    sources[0]!.emit("snapshot", "7.1", board([]));
    sources[0]!.fail(true);
    setHidden(true);
    expect(sources[0]!.closed).toBe(true);
    expect(timers.every((timer) => timer.cancelled)).toBe(true);
    expect(sources).toHaveLength(1);
    setHidden(false);
    expect(sources).toHaveLength(2);
    // The new stream starts over, so its first patch must follow its own snapshot.
    sources[1]!.emit("snapshot", "9.3", board([]));
    sources[1]!.emit("patch", "9.4", { base: "9.3", patch: patch(9) });
    expect(handlers.snapshot).toHaveBeenCalledTimes(2);
    expect(handlers.patch).toHaveBeenCalledTimes(1);
  });

  it("waits for a hidden tab to become visible before connecting", () => {
    const { stream, sources, setHidden } = setup(true);
    stream.start();
    expect(sources).toHaveLength(0);
    setHidden(false);
    expect(sources).toHaveLength(1);
    stream.stop();
    expect(sources[0]!.closed).toBe(true);
    setHidden(true);
    setHidden(false);
    expect(sources).toHaveLength(1);
  });
});
