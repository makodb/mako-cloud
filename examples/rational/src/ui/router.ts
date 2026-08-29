import { useEffect, useState } from "react";

/** A hash router: no dependency, works from any static host. */
export type Route =
  | { readonly name: "accounts" }
  | { readonly name: "transactions"; readonly accountId?: string; readonly month?: string }
  | { readonly name: "categories" }
  | { readonly name: "tags" }
  | { readonly name: "household" };

export function parseRoute(hash: string): Route {
  const [path = "", query = ""] = hash.replace(/^#\/?/u, "").split("?");
  const parameters = new URLSearchParams(query);
  switch (path) {
    case "transactions": {
      const accountId = parameters.get("account");
      const month = parameters.get("month");
      return {
        name: "transactions",
        ...(accountId === null || accountId === "" ? {} : { accountId }),
        ...(month === null || month === "" ? {} : { month }),
      };
    }
    case "categories":
      return { name: "categories" };
    case "tags":
      return { name: "tags" };
    case "household":
      return { name: "household" };
    default:
      return { name: "accounts" };
  }
}

export function routeHash(route: Route): string {
  if (route.name === "transactions") {
    const parameters = new URLSearchParams();
    if (route.accountId !== undefined) parameters.set("account", route.accountId);
    if (route.month !== undefined) parameters.set("month", route.month);
    const query = parameters.toString();
    return `#/transactions${query === "" ? "" : `?${query}`}`;
  }
  return `#/${route.name}`;
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
