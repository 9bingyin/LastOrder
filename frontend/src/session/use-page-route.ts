import { useEffect } from "react";
import { errorMessage } from "../api";
import { recordDebug } from "../debug/diagnostics";

export function usePageRoute(
  token: string | null,
  setPage: (page: "/" | "/files" | "/debug") => void,
  setQualityOpen: (open: boolean) => void,
) {
  useEffect(() => {
    const pop = () => {
      const path = window.location.pathname;
      setPage(path === "/files" || path === "/debug" ? path : "/");
      setQualityOpen(false);
    };
    const error = (event: ErrorEvent) =>
      recordDebug(
        "window.error",
        {
          message: event.message,
          stack: event.error instanceof Error ? event.error.stack : null,
        },
        token ? [token] : [],
      );
    const rejection = (event: PromiseRejectionEvent) =>
      recordDebug(
        "window.unhandledrejection",
        { message: errorMessage(event.reason) },
        token ? [token] : [],
      );
    window.addEventListener("popstate", pop);
    window.addEventListener("error", error);
    window.addEventListener("unhandledrejection", rejection);
    return () => {
      window.removeEventListener("popstate", pop);
      window.removeEventListener("error", error);
      window.removeEventListener("unhandledrejection", rejection);
    };
  }, [token, setPage, setQualityOpen]);
}
