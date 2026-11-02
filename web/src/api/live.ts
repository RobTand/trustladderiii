import { skipToken, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { applyPatch } from "./board";
import { BoardStream, browserEnvironment, type StreamState } from "./stream";
import type { Board } from "./types.gen";

export type Connection = "connecting" | "live" | "reconnecting" | "database-offline";

const BOARD = ["board"] as const;

/**
 * The board, kept current by the console's event stream alone: every
 * connection starts with a snapshot, so no separate fetch can race it.
 */
export function useLiveBoard(): { board: Board | undefined; connection: Connection } {
  const client = useQueryClient();
  const { data: board } = useQuery<Board>({ queryKey: BOARD, queryFn: skipToken });
  const [stream, setStream] = useState<StreamState>("connecting");
  const [database, setDatabase] = useState(true);
  useEffect(() => {
    const live = new BoardStream(
      {
        snapshot: (next) => client.setQueryData<Board>(BOARD, next),
        patch: (patch) => client.setQueryData<Board>(BOARD, (shown) => (shown ? applyPatch(shown, patch) : shown)),
        database: setDatabase,
        state: setStream,
      },
      browserEnvironment(),
    );
    live.start();
    return () => live.stop();
  }, [client]);
  const connection: Connection =
    stream === "open" ? (database ? "live" : "database-offline") : stream === "connecting" ? "connecting" : "reconnecting";
  return { board, connection };
}
