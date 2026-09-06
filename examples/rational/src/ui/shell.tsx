import { type ReactNode, useEffect, useState } from "react";

import type { AppState, RationalApp } from "../data/rational.js";
import { firedAlertHistory, unreadCount } from "../selectors/alerts.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash } from "./router.js";
import "./styles/shell.css";

/**
 * Monarch's shape: a sidebar of the places money lives, a top bar for the
 * space, the notifications, and the person. Settings gathers everything that
 * configures the household rather than shows its money.
 */
const NAV: ReadonlyArray<{ readonly route: Route; readonly label: string; readonly icon: string }> =
  [
    { route: { name: "dashboard" }, label: "Dashboard", icon: "◫" },
    { route: { name: "accounts" }, label: "Accounts", icon: "◆" },
    { route: { name: "transactions", params: {} }, label: "Transactions", icon: "≡" },
    { route: { name: "cash-flow" }, label: "Cash Flow", icon: "⇅" },
    { route: { name: "budget" }, label: "Budget", icon: "◔" },
    { route: { name: "recurring" }, label: "Recurring", icon: "↻" },
    { route: { name: "goals" }, label: "Goals", icon: "◎" },
    { route: { name: "investments" }, label: "Investments", icon: "△" },
    { route: { name: "settings", page: "household" }, label: "Settings", icon: "⚙" },
  ];

/** Which sidebar entry a route belongs to: an account page is still "Accounts". */
function sectionOf(route: Route): Route["name"] {
  return route.name === "account" ? "accounts" : route.name;
}

type Theme = "light" | "dark";
const THEME_KEY = "rational.theme";

function readTheme(): Theme {
  try {
    return window.localStorage.getItem(THEME_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

/** The theme is a per-device preference, applied to the document root so CSS tokens follow it. */
export function useTheme(): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(readTheme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      window.localStorage.setItem(THEME_KEY, theme);
    } catch {
      // Not remembering is fine.
    }
  }, [theme]);
  return [theme, () => setTheme(theme === "dark" ? "light" : "dark")];
}

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
  const [theme, toggleTheme] = useTheme();
  const section = sectionOf(route);
  return (
    <div className="shell">
      <aside className="sidebar">
        <a className="brand" href={routeHash({ name: "dashboard" })}>
          <span className="eyebrow">Rational</span>
        </a>
        <nav aria-label="Sections">
          {NAV.map((entry) => (
            <a
              key={entry.route.name}
              href={routeHash(entry.route)}
              aria-current={section === entry.route.name ? "page" : undefined}
            >
              <span className="nav-icon" aria-hidden="true">
                {entry.icon}
              </span>
              {entry.label}
            </a>
          ))}
        </nav>
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
      </aside>
      <div className="main">
        <header className="topbar">
          {/* One space is the ordinary case and needs no chrome; the picker
              appears only for people who actually belong to more than one. */}
          {state.memberships.length > 1 ? (
            <label className="switcher">
              <span className="visually-hidden">Space</span>
              <select
                aria-label="Space"
                value={state.currentHouseholdId ?? ""}
                onChange={(event) => void app.selectHousehold(event.target.value || null)}
              >
                {state.memberships.map((membership) => (
                  <option key={membership.household_id} value={membership.household_id}>
                    {state.households.find((candidate) => candidate.id === membership.household_id)
                      ?.name ?? membership.household_id}{" "}
                    · {membership.role}
                  </option>
                ))}
              </select>
            </label>
          ) : (
            <span className="space-name">
              {state.households.find((candidate) => candidate.id === state.currentHouseholdId)
                ?.name ?? ""}
            </span>
          )}
          <div className="session">
            <NotificationBell app={app} />
            <button
              type="button"
              className="icon"
              aria-label={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
              aria-pressed={theme === "dark"}
              data-testid="theme-toggle"
              onClick={toggleTheme}
            >
              {theme === "dark" ? "☾" : "☼"}
            </button>
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
      </div>
    </div>
  );
}

/**
 * The bell: how many alerts nobody has read, and the latest few. Alerts are
 * documents like any other, so the count is live wherever the person is.
 */
function NotificationBell({ app }: { app: RationalApp }) {
  const session = app.household?.session ?? null;
  const documents = useQuery(session?.collection("alerts")?.find() ?? null);
  const [open, setOpen] = useState(false);
  const unread = unreadCount(documents);
  const latest = firedAlertHistory(documents).slice(0, 5);
  return (
    <div className="bell">
      <button
        type="button"
        className="icon"
        aria-label={unread === 0 ? "Notifications" : `Notifications, ${unread} unread`}
        aria-expanded={open}
        data-testid="notification-bell"
        onClick={() => setOpen(!open)}
      >
        ♪
        {unread === 0 ? null : (
          <span className="badge" data-testid="unread-count">
            {unread}
          </span>
        )}
      </button>
      {open ? (
        <div className="popover" role="dialog" aria-label="Notifications">
          {latest.length === 0 ? (
            <p className="muted">Nothing yet.</p>
          ) : (
            <ul>
              {latest.map((alert) => (
                <li key={alert.id} data-read={alert.read === true ? "yes" : "no"}>
                  <span>{alert.message}</span>
                  {alert.read === true ? null : (
                    <button
                      type="button"
                      className="link"
                      onClick={() => void app.writes?.markAlertRead(alert.id, true)}
                    >
                      Mark read
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
          <a href={routeHash({ name: "settings", page: "notifications" })}>All notifications</a>
        </div>
      ) : null}
    </div>
  );
}
