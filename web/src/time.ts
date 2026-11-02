import { useEffect, useState } from "react";

/** "45s", "12m", "3h 05m", "2d 4h". */
export function elapsed(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/**
 * The wall clock for relative times on screen, refreshed each minute while
 * the tab is visible. It only redraws labels; board data changes arrive on
 * the event stream, which also pauses while the tab is hidden.
 */
export function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    let timer: number | undefined;
    const follow = () => {
      window.clearInterval(timer);
      timer = undefined;
      if (document.hidden) return;
      setNow(Date.now());
      timer = window.setInterval(() => setNow(Date.now()), 60_000);
    };
    follow();
    document.addEventListener("visibilitychange", follow);
    return () => {
      document.removeEventListener("visibilitychange", follow);
      window.clearInterval(timer);
    };
  }, []);
  return now;
}
