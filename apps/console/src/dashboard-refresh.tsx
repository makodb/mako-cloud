import { ManagementApiError } from "@mako-cloud/management-sdk";
import { useCallback, useEffect, useRef, useState } from "react";
import { type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";

export const DASHBOARD_REFRESH_MS = 60_000;

export type DashboardState<T> =
  | { readonly status: "loading" }
  | {
      readonly status: "ready";
      readonly value: T;
      readonly observedAt: Date;
      readonly refreshing: boolean;
      readonly refreshFailure: ConsoleApiFailure | null;
    }
  | { readonly status: "unavailable"; readonly failure: ConsoleApiFailure };

/** Memoize load so the polling lifetime follows the requested project/environment. */
export function useDashboardResource<T>(load: () => Promise<T>) {
  const [snapshot, setSnapshot] = useState<{ load: typeof load; state: DashboardState<T> }>({
    load,
    state: { status: "loading" },
  });
  const refreshRef = useRef<() => void>(() => {});
  useEffect(() => {
    let active = true;
    let pending = false;
    setSnapshot({ load, state: { status: "loading" } });
    const refresh = async () => {
      if (!active || pending) return;
      pending = true;
      setSnapshot((previous) => ({
        load,
        state:
          previous.load === load && previous.state.status === "ready"
            ? { ...previous.state, refreshing: true }
            : { status: "loading" },
      }));
      try {
        const value = await load();
        if (active)
          setSnapshot({
            load,
            state: {
              status: "ready",
              value,
              observedAt: new Date(),
              refreshing: false,
              refreshFailure: null,
            },
          });
      } catch (error) {
        if (active) {
          const failure = toConsoleApiFailure(error);
          const accessLost =
            error instanceof ManagementApiError && [401, 403, 404].includes(error.status);
          setSnapshot((previous) => ({
            load,
            state:
              !accessLost && previous.load === load && previous.state.status === "ready"
                ? { ...previous.state, refreshing: false, refreshFailure: failure }
                : { status: "unavailable", failure },
          }));
        }
      } finally {
        pending = false;
      }
    };
    refreshRef.current = () => void refresh();
    void refresh();
    const timer = window.setInterval(() => void refresh(), DASHBOARD_REFRESH_MS);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [load]);
  const refresh = useCallback(() => refreshRef.current(), []);
  // Never render a previous environment's snapshot while its effect cleans up.
  const state: DashboardState<T> = snapshot.load === load ? snapshot.state : { status: "loading" };
  return { state, refresh };
}

export function UpdatedAt({ at }: { readonly at: Date | string | number | null }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 10_000);
    return () => window.clearInterval(timer);
  }, []);
  const timestamp = at === null ? Number.NaN : new Date(at).getTime();
  if (!Number.isFinite(timestamp)) return <span>Update time unavailable</span>;
  const seconds = Math.max(0, Math.floor((now - timestamp) / 1000));
  const [amount, unit] =
    seconds < 60
      ? [seconds, "second"]
      : seconds < 3600
        ? [Math.floor(seconds / 60), "minute"]
        : seconds < 86400
          ? [Math.floor(seconds / 3600), "hour"]
          : [Math.floor(seconds / 86400), "day"];
  return (
    <time dateTime={new Date(timestamp).toISOString()} title={new Date(timestamp).toLocaleString()}>
      {seconds < 5 ? "Updated just now" : `Updated ${amount} ${unit}${amount === 1 ? "" : "s"} ago`}
    </time>
  );
}
