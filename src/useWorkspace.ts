import { useCallback, useEffect, useRef, useState } from "react";
import { command, errorText } from "./api";
import {
  EMPTY_SUMMARY,
  type Query,
  type QueryResult,
  type Summary,
} from "./types";

export function useWorkspace(query: Query) {
  const [summary, setSummary] = useState(EMPTY_SUMMARY);
  const [result, setResult] = useState<QueryResult>({
    entries: [],
    total: 0,
    revision: 0,
    offset: 0,
  });
  const [connected, setConnected] = useState(false);
  const [error, setErrorState] = useState<{
    message: string;
    source: "summary" | "query" | "action";
  } | null>(null);
  const setError = useCallback(
    (message: string) =>
      setErrorState(message ? { message, source: "action" } : null),
    [],
  );
  const [loading, setLoading] = useState(true);
  const [refreshKey, setRefreshKey] = useState(0);
  const [reconnectKey, setReconnectKey] = useState(0);
  const connectionFailed = useRef(false);
  const refresh = useCallback(() => setRefreshKey((key) => key + 1), []);
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const controller = new AbortController();
    async function poll() {
      try {
        const next = await command<Summary>(
          "summary",
          { scopeId: query.scopeId, hidden: query.hidden },
          controller.signal,
        );
        if (!stopped) {
          setSummary((previous) =>
            previous.facetScopeId === next.facetScopeId &&
            previous.facetHidden === next.facetHidden &&
            previous.revision === next.revision &&
            previous.scanPaused === next.scanPaused &&
            JSON.stringify(previous.scanRuns) ===
              JSON.stringify(next.scanRuns) &&
            previous.scopes.length === next.scopes.length &&
            previous.scopes.every(
              (scope, index) =>
                scope.progress?.elapsedMs ===
                next.scopes[index]?.progress?.elapsedMs,
            )
              ? previous
              : next,
          );
          setConnected(true);
          setErrorState((current) =>
            current?.source === "summary" ? null : current,
          );
          if (connectionFailed.current) {
            connectionFailed.current = false;
            setReconnectKey((key) => key + 1);
          }
        }
      } catch (error) {
        if (!stopped) {
          connectionFailed.current = true;
          setConnected(false);
          setErrorState({ message: errorText(error), source: "summary" });
        }
      }
      if (!stopped) timer = setTimeout(poll, 1200);
    }
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
      controller.abort();
    };
    // The polling loop has one owner, including when the connection recovers.
  }, [refreshKey, query.scopeId, query.hidden]);
  const sequence = useRef(0);
  useEffect(() => {
    const current = ++sequence.current;
    const controller = new AbortController();
    setLoading(true);
    command<QueryResult>("query", query, controller.signal)
      .then((next) => {
        if (sequence.current === current) {
          setResult(next);
          setLoading(false);
          setErrorState((current) =>
            current?.source === "query" ? null : current,
          );
        }
      })
      .catch((error) => {
        if (!controller.signal.aborted && sequence.current === current) {
          setErrorState({ message: errorText(error), source: "query" });
          setLoading(false);
        }
      });
    return () => {
      controller.abort();
      sequence.current++;
    };
  }, [query, summary.revision, refreshKey, reconnectKey]);
  return {
    summary,
    result,
    connected,
    loading,
    error: error?.message ?? "",
    setError,
    refresh,
  };
}
