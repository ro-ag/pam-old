import { useCallback, useSyncExternalStore } from "react";

/** Wide-desktop breakpoint: list+detail panels render side by side. */
export const WIDE_VIEWPORT_QUERY = "(min-width: 1360px)";

export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback((onChange: () => void) => {
    const list = window.matchMedia(query);
    list.addEventListener("change", onChange);
    return () => list.removeEventListener("change", onChange);
  }, [query]);
  return useSyncExternalStore(subscribe, () => window.matchMedia(query).matches);
}
