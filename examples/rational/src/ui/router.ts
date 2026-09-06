import { useEffect, useState } from "react";

/**
 * A hash router: no dependency, works from any static host.
 *
 * Every screen's state that a person would want back after a reload — the
 * account they were looking at, the month, the range, every transaction
 * filter — lives in the address, so a link to a screen is a link to exactly
 * what was on it.
 */
export const SETTINGS_PAGES = [
  "household",
  "members",
  "categories",
  "tags",
  "merchants",
  "rules",
  "connections",
  "import",
  "notifications",
  "data",
] as const;
export type SettingsPage = (typeof SETTINGS_PAGES)[number];

export type Route =
  | { readonly name: "dashboard" }
  | { readonly name: "accounts" }
  | { readonly name: "account"; readonly accountId: string }
  | { readonly name: "transactions"; readonly params: Readonly<Record<string, string>> }
  | { readonly name: "cash-flow"; readonly range?: string; readonly view?: string }
  | { readonly name: "budget"; readonly month?: string }
  | { readonly name: "recurring"; readonly month?: string; readonly view?: "list" | "calendar" }
  | { readonly name: "goals" }
  | { readonly name: "investments" }
  | { readonly name: "settings"; readonly page: SettingsPage };

/** The routes of the earlier Rational, kept alive as redirects. */
const LEGACY: Readonly<Record<string, Route>> = {
  reports: { name: "cash-flow" },
  budgets: { name: "budget" },
  plan: { name: "recurring" },
  connections: { name: "settings", page: "connections" },
  import: { name: "settings", page: "import" },
  rules: { name: "settings", page: "rules" },
  alerts: { name: "settings", page: "notifications" },
  categories: { name: "settings", page: "categories" },
  tags: { name: "settings", page: "tags" },
  household: { name: "settings", page: "members" },
};

function optional<Key extends string>(
  key: Key,
  value: string | null,
): Partial<Record<Key, string>> {
  return value === null || value === "" ? {} : ({ [key]: value } as Record<Key, string>);
}

export function parseRoute(hash: string): Route {
  const [rawPath = "", query = ""] = hash.replace(/^#\/?/u, "").split("?");
  const path = decodeURIComponent(rawPath);
  const parameters = new URLSearchParams(query);
  const [head = "", tail = ""] = path.split("/");
  switch (head) {
    case "":
    case "dashboard":
      return { name: "dashboard" };
    case "accounts":
      return tail === "" ? { name: "accounts" } : { name: "account", accountId: tail };
    case "transactions": {
      const params: Record<string, string> = {};
      for (const [key, value] of parameters) {
        if (value !== "") params[key] = value;
      }
      return { name: "transactions", params };
    }
    case "cash-flow":
      return {
        name: "cash-flow",
        ...optional("range", parameters.get("range")),
        ...optional("view", parameters.get("view")),
      };
    case "budget":
      return { name: "budget", ...optional("month", parameters.get("month")) };
    case "recurring": {
      const view = parameters.get("view");
      return {
        name: "recurring",
        ...optional("month", parameters.get("month")),
        ...(view === "calendar" ? { view: "calendar" as const } : {}),
      };
    }
    case "goals":
      return { name: "goals" };
    case "investments":
      return { name: "investments" };
    case "settings":
      return {
        name: "settings",
        page: (SETTINGS_PAGES as readonly string[]).includes(tail)
          ? (tail as SettingsPage)
          : "household",
      };
    default: {
      const legacy = LEGACY[head];
      if (legacy === undefined) return { name: "dashboard" };
      // The old month parameter meant the same thing on both screens it was on.
      if (legacy.name === "budget") {
        return { name: "budget", ...optional("month", parameters.get("month")) };
      }
      if (legacy.name === "cash-flow") {
        const month = parameters.get("month");
        return month === null || month === "" ? legacy : { name: "cash-flow", range: month };
      }
      return legacy;
    }
  }
}

function withQuery(path: string, parameters: URLSearchParams): string {
  const query = parameters.toString();
  return query === "" ? path : `${path}?${query}`;
}

export function routeHash(route: Route): string {
  switch (route.name) {
    case "dashboard":
      return "#/dashboard";
    case "account":
      return `#/accounts/${encodeURIComponent(route.accountId)}`;
    case "transactions": {
      const parameters = new URLSearchParams();
      for (const [key, value] of Object.entries(route.params)) {
        if (value !== "") parameters.set(key, value);
      }
      return withQuery("#/transactions", parameters);
    }
    case "cash-flow": {
      const parameters = new URLSearchParams();
      if (route.range !== undefined) parameters.set("range", route.range);
      if (route.view !== undefined) parameters.set("view", route.view);
      return withQuery("#/cash-flow", parameters);
    }
    case "budget": {
      const parameters = new URLSearchParams();
      if (route.month !== undefined) parameters.set("month", route.month);
      return withQuery("#/budget", parameters);
    }
    case "recurring": {
      const parameters = new URLSearchParams();
      if (route.month !== undefined) parameters.set("month", route.month);
      if (route.view !== undefined) parameters.set("view", route.view);
      return withQuery("#/recurring", parameters);
    }
    case "settings":
      return `#/settings/${route.page}`;
    default:
      return `#/${route.name}`;
  }
}

/** The transactions screen with these filters, for links from other screens. */
export function transactionsHash(params: Readonly<Record<string, string>>): string {
  return routeHash({ name: "transactions", params });
}

export function useRoute(): [Route, (route: Route) => void] {
  const [route, setRoute] = useState<Route>(() => parseRoute(window.location.hash));
  useEffect(() => {
    const onChange = () => setRoute(parseRoute(window.location.hash));
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);
  const navigate = (next: Route) => {
    window.location.hash = routeHash(next);
  };
  return [route, navigate];
}
