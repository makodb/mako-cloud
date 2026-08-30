import type { ReactNode } from "react";

import type { AppState, RationalApp } from "../data/rational.js";
import { type Route, routeHash } from "./router.js";

const NAV: ReadonlyArray<{ readonly route: Route; readonly label: string }> = [
  { route: { name: "accounts" }, label: "Accounts" },
  { route: { name: "transactions" }, label: "Transactions" },
  { route: { name: "budgets" }, label: "Budgets" },
  { route: { name: "plan" }, label: "Plan" },
  { route: { name: "import" }, label: "Import" },
  { route: { name: "connections" }, label: "Connections" },
  { route: { name: "rules" }, label: "Rules" },
  { route: { name: "alerts" }, label: "Alerts" },
  { route: { name: "reports" }, label: "Reports" },
  { route: { name: "categories" }, label: "Categories" },
  { route: { name: "tags" }, label: "Tags" },
  { route: { name: "household" }, label: "Household" },
];

export function Shell({
  app,
  state,
  route,
  children,
}: {
  app: RationalApp;
  state: AppState;
  route: Route;
  children: ReactNode;
}) {
  const household = state.household;
  const offline = state.connectivity !== "online";
  const pending = household?.pendingWrites ?? 0;
  return (
    <div className="shell">
      <header className="topbar">
        <div className="brand">
          <span className="eyebrow">Rational</span>
          <label className="switcher">
            <span className="visually-hidden">Household</span>
            <select
              aria-label="Household"
              value={state.currentHouseholdId ?? ""}
              onChange={(event) => void app.selectHousehold(event.target.value || null)}
            >
              {state.memberships.length === 0 ? <option value="">No households yet</option> : null}
              {state.memberships.map((membership) => (
                <option key={membership.household_id} value={membership.household_id}>
                  {state.households.find((candidate) => candidate.id === membership.household_id)
                    ?.name ?? membership.household_id}{" "}
                  · {membership.role}
                </option>
              ))}
            </select>
          </label>
        </div>
        <nav aria-label="Sections">
          {NAV.map((entry) => (
            <a
              key={entry.route.name}
              href={routeHash(entry.route)}
              aria-current={route.name === entry.route.name ? "page" : undefined}
            >
              {entry.label}
            </a>
          ))}
        </nav>
        <div className="session">
          <span className="user">{state.user?.email}</span>
          <button type="button" className="secondary" onClick={() => void app.signOut()}>
            Sign out
          </button>
        </div>
      </header>
      {offline ? (
        <div className="banner offline" role="status" data-testid="offline-banner">
          {state.connectivity === "offline" ? "You're offline." : "The service is unreachable."}{" "}
          Changes are saved on this device and will sync when you're back online.
          {pending > 0 ? (
            <strong data-testid="pending-writes">
              {" "}
              {pending} {pending === 1 ? "change" : "changes"} waiting
            </strong>
          ) : null}
        </div>
      ) : null}
      {household !== null && household.recovery.kind !== "active" ? (
        <div className="banner blocking" role="alert">
          {household.notice ?? "Replication needs attention."}
        </div>
      ) : null}
      {state.notice !== null ||
      (household?.notice !== null && household?.recovery.kind === "active") ? (
        <div className="banner notice" role="status" data-testid="notice">
          {state.notice ?? household?.notice}
        </div>
      ) : null}
      <main id="main-content" className="content">
        {children}
      </main>
      <footer className="statusbar">
        <span data-testid="sync-activity">Sync: {household?.activity ?? "idle"}</span>
        <span>
          {household?.syncedAt === null || household === null
            ? "not synced yet"
            : `as of ${new Date(household.syncedAt).toLocaleTimeString()}`}
        </span>
        <span data-testid="network-toggle">
          <button type="button" className="link" onClick={() => void app.setOnline(false)}>
            Go offline
          </button>
          <button type="button" className="link" onClick={() => void app.setOnline(true)}>
            Go online
          </button>
        </span>
      </footer>
    </div>
  );
}
