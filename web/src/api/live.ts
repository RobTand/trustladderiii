import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { applyPatch } from "./board";
import type { Board, BoardPatch, DatabaseStatus } from "./types.gen";

export type Connection = "connecting" | "live" | "reconnecting" | "database-offline";

const BOARD = ["board"] as const;

async function fetchBoard(): Promise<Board> {
  const response = await fetch("/api/board", { headers: { Accept: "application/json" } });
  if (!response.ok) throw new Error(`The console answered ${response.status}`);
  return (await response.json()) as Board;
}

/**
 * The board, kept current by the console's event stream. Every (re)connection
 * starts with a full snapshot; the browser's EventSource reconnects by itself
 * after the delay the console sets.
 */
export function useLiveBoard(): { board: Board | undefined; connection: Connection; error: Error | null } {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: BOARD,
    queryFn: fetchBoard,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
  });
  const [stream, setStream] = useState<"connecting" | "open" | "closed">("connecting");
  const [database, setDatabase] = useState(true);
  useEffect(() => {
    const source = new EventSource("/api/stream");
    source.onopen = () => setStream("open");
    source.onerror = () => setStream(source.readyState === EventSource.CLOSED ? "closed" : "connecting");
    source.addEventListener("snapshot", (event) => {
      client.setQueryData<Board>(BOARD, JSON.parse((event as MessageEvent<string>).data) as Board);
    });
    source.addEventListener("patch", (event) => {
      const patch = JSON.parse((event as MessageEvent<string>).data) as BoardPatch;
      client.setQueryData<Board>(BOARD, (board) => (board ? applyPatch(board, patch) : board));
    });
    source.addEventListener("database", (event) => {
      setDatabase((JSON.parse((event as MessageEvent<string>).data) as DatabaseStatus).connected);
    });
    return () => source.close();
  }, [client]);
  const connection: Connection =
    stream === "open" ? (database ? "live" : "database-offline") : stream === "connecting" && !query.data ? "connecting" : "reconnecting";
  return { board: query.data, connection, error: query.data ? null : query.error };
}
