import { useEffect, useState } from "react";

export type Theme = "system" | "light" | "dark";
const KEY = "tl3-console-theme";

function stored(): Theme {
  const value = window.localStorage.getItem(KEY);
  return value === "light" || value === "dark" ? value : "system";
}

/** System follows prefers-color-scheme; light and dark pin the tokens. */
export function useTheme(): [Theme, (theme: Theme) => void] {
  const [theme, setTheme] = useState<Theme>(stored);
  useEffect(() => {
    const root = document.documentElement;
    if (theme === "system") {
      delete root.dataset.theme;
      window.localStorage.removeItem(KEY);
    } else {
      root.dataset.theme = theme;
      window.localStorage.setItem(KEY, theme);
    }
  }, [theme]);
  return [theme, setTheme];
}
