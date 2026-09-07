import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  NativeSelect,
  Popover,
  PopoverContent,
  PopoverTrigger,
  ThemeToggle,
  cn,
  useTheme,
} from "@mako-cloud/ui";
import {
  ArrowLeftRight,
  Bell,
  Landmark,
  LayoutDashboard,
  type LucideIcon,
  PieChart,
  ReceiptText,
  Repeat,
  Settings,
  Target,
  TrendingUp,
} from "lucide-react";
import type { ReactNode } from "react";

import type { AppState, RationalApp } from "../data/rational.js";
import { firedAlertHistory, unreadCount } from "../selectors/alerts.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash } from "./router.js";

/**
 * Monarch's shape: a sidebar of the places money lives, a top bar for the
 * space, the notifications, and the person. Settings gathers everything that
 * configures the household rather than shows its money.
 */
const NAV: ReadonlyArray<{
  readonly route: Route;
  readonly label: string;
  readonly icon: LucideIcon;
}> = [
  { route: { name: "dashboard" }, label: "Dashboard", icon: LayoutDashboard },
  { route: { name: "accounts" }, label: "Accounts", icon: Landmark },
  { route: { name: "transactions", params: {} }, label: "Transactions", icon: ReceiptText },
  { route: { name: "cash-flow" }, label: "Cash Flow", icon: ArrowLeftRight },
  { route: { name: "budget" }, label: "Budget", icon: PieChart },
  { route: { name: "recurring" }, label: "Recurring", icon: Repeat },
  { route: { name: "goals" }, label: "Goals", icon: Target },
  { route: { name: "investments" }, label: "Investments", icon: TrendingUp },
  { route: { name: "settings", page: "household" }, label: "Settings", icon: Settings },
];

/** Which sidebar entry a route belongs to: an account page is still "Accounts". */
function sectionOf(route: Route): Route["name"] {
  return route.name === "account" ? "accounts" : route.name;
}

/** The per-device theme preference lives under this key, as it always has. */
export const THEME_KEY = "rational.theme";

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
  const { resolved, toggle } = useTheme(THEME_KEY);
  const section = sectionOf(route);
  const spaceName =
    state.households.find((candidate) => candidate.id === state.currentHouseholdId)?.name ?? "";
  return (
    <div className="grid min-h-screen grid-cols-[15rem_minmax(0,1fr)]">
      <aside className="sticky top-0 flex h-screen flex-col gap-6 border-r bg-sidebar px-3 py-5 text-sidebar-foreground">
        <a
          className="px-3 text-xs font-semibold tracking-[0.18em] text-primary uppercase no-underline hover:no-underline"
          href={routeHash({ name: "dashboard" })}
        >
          Rational
        </a>
        <nav aria-label="Sections" className="grid gap-0.5">
          {NAV.map((entry) => {
            const active = section === entry.route.name;
            const Icon = entry.icon;
            return (
              <a
                key={entry.route.name}
                href={routeHash(entry.route)}
                aria-current={active ? "page" : undefined}
                className={cn(
                  "flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-muted-foreground no-underline transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:no-underline",
                  active && "bg-sidebar-accent text-sidebar-foreground",
                )}
              >
                <Icon aria-hidden="true" className="size-4 shrink-0" />
                {entry.label}
              </a>
            );
          })}
        </nav>
        <footer className="mt-auto grid gap-1 px-3 text-xs text-muted-foreground">
          <span data-testid="sync-activity">Sync: {household?.activity ?? "idle"}</span>
          <span>
            {household?.syncedAt === null || household === null
              ? "not synced yet"
              : `as of ${new Date(household.syncedAt).toLocaleTimeString()}`}
          </span>
          <span data-testid="network-toggle" className="flex gap-3">
            <Button
              variant="link"
              size="sm"
              className="h-auto p-0 text-xs"
              onClick={() => void app.setOnline(false)}
            >
              Go offline
            </Button>
            <Button
              variant="link"
              size="sm"
              className="h-auto p-0 text-xs"
              onClick={() => void app.setOnline(true)}
            >
              Go online
            </Button>
          </span>
        </footer>
      </aside>
      <div className="flex min-w-0 flex-col">
        <header className="flex h-14 items-center gap-3 border-b bg-card px-6">
          {/* One space is the ordinary case and needs no chrome; the picker
              appears only for people who actually belong to more than one. */}
          {state.memberships.length > 1 ? (
            <NativeSelect
              aria-label="Space"
              size="sm"
              className="w-auto min-w-48 font-medium"
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
            </NativeSelect>
          ) : (
            <span className="font-medium">{spaceName}</span>
          )}
          <div className="ml-auto flex items-center gap-2">
            <NotificationBell app={app} />
            <ThemeToggle resolved={resolved} onToggle={toggle} data-testid="theme-toggle" />
            <span className="max-w-56 truncate text-sm text-muted-foreground">
              {state.user?.email}
            </span>
            <Button variant="outline" size="sm" onClick={() => void app.signOut()}>
              Sign out
            </Button>
          </div>
        </header>
        {offline ? (
          <Alert
            variant="warning"
            role="status"
            data-testid="offline-banner"
            className="rounded-none border-x-0 border-t-0"
          >
            <AlertDescription className="block">
              {state.connectivity === "offline" ? "You're offline." : "The service is unreachable."}{" "}
              Changes are saved on this device and will sync when you're back online.
              {pending > 0 ? (
                <strong data-testid="pending-writes">
                  {" "}
                  {pending} {pending === 1 ? "change" : "changes"} waiting
                </strong>
              ) : null}
            </AlertDescription>
          </Alert>
        ) : null}
        {household !== null && household.recovery.kind !== "active" ? (
          <Alert variant="destructive" role="alert" className="rounded-none border-x-0 border-t-0">
            <AlertDescription className="block">
              {household.notice ?? "Replication needs attention."}
            </AlertDescription>
          </Alert>
        ) : null}
        {state.notice !== null ||
        (household?.notice !== null && household?.recovery.kind === "active") ? (
          <Alert role="status" data-testid="notice" className="rounded-none border-x-0 border-t-0">
            <AlertDescription className="block text-foreground">
              {state.notice ?? household?.notice}
            </AlertDescription>
          </Alert>
        ) : null}
        <main id="main-content" className="flex-1 px-6 py-6">
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
  const unread = unreadCount(documents);
  const latest = firedAlertHistory(documents).slice(0, 5);
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="relative"
          aria-label={unread === 0 ? "Notifications" : `Notifications, ${unread} unread`}
          data-testid="notification-bell"
        >
          <Bell />
          {unread === 0 ? null : (
            <Badge
              data-testid="unread-count"
              className="absolute -top-0.5 -right-0.5 h-4 min-w-4 px-1 text-[10px] leading-none"
            >
              {unread}
            </Badge>
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" aria-label="Notifications" className="w-80 p-3">
        {latest.length === 0 ? (
          <p className="text-sm text-muted-foreground">Nothing yet.</p>
        ) : (
          <ul className="m-0 grid list-none gap-2 p-0">
            {latest.map((alert) => (
              <li
                key={alert.id}
                data-read={alert.read === true ? "yes" : "no"}
                className={cn(
                  "flex items-start justify-between gap-3 rounded-md px-2 py-1.5 text-sm",
                  alert.read === true ? "text-muted-foreground" : "bg-accent/60",
                )}
              >
                <span>{alert.message}</span>
                {alert.read === true ? null : (
                  <Button
                    variant="link"
                    size="sm"
                    className="h-auto shrink-0 p-0 text-xs"
                    onClick={() => void app.writes?.markAlertRead(alert.id, true)}
                  >
                    Mark read
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
        <a
          className="mt-3 block text-sm"
          href={routeHash({ name: "settings", page: "notifications" })}
        >
          All notifications
        </a>
      </PopoverContent>
    </Popover>
  );
}
