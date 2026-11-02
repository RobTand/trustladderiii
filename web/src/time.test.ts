import { act, renderHook } from "@testing-library/react";
import { useNow } from "./time";

describe("useNow", () => {
  let hidden = false;
  beforeEach(() => {
    vi.useFakeTimers({ now: 0 });
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  });
  afterEach(() => {
    vi.useRealTimers();
    hidden = false;
  });

  const show = (value: boolean) =>
    act(() => {
      hidden = !value;
      document.dispatchEvent(new Event("visibilitychange"));
    });

  it("ticks each minute while visible and stops while the tab is hidden", () => {
    const { result } = renderHook(() => useNow());
    expect(result.current).toBe(0);
    act(() => vi.advanceTimersByTime(60_000));
    expect(result.current).toBe(60_000);
    show(false);
    expect(vi.getTimerCount()).toBe(0);
    act(() => vi.advanceTimersByTime(10 * 60_000));
    expect(result.current).toBe(60_000);
    show(true);
    expect(result.current).toBe(11 * 60_000);
    act(() => vi.advanceTimersByTime(60_000));
    expect(result.current).toBe(12 * 60_000);
  });
});
