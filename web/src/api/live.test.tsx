import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { useLiveBoard } from "./live";

describe("useLiveBoard", () => {
  it("takes the board from the stream alone, so a slow fetch can't replace a newer snapshot", () => {
    const opened: string[] = [];
    class StubSource {
      readyState = 0;
      onopen = null;
      onerror = null;
      constructor(url: string) {
        opened.push(url);
      }
      addEventListener() {}
      close() {}
    }
    vi.stubGlobal("EventSource", StubSource);
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const client = new QueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useLiveBoard(), { wrapper });
    expect(opened).toEqual(["/api/stream"]);
    expect(fetch).not.toHaveBeenCalled();
    expect(result.current.board).toBeUndefined();
    vi.unstubAllGlobals();
  });
});
