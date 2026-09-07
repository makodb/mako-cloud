import { Alert, AlertDescription } from "@mako-cloud/ui";
import { Component, type ReactNode } from "react";

import type { RationalApp } from "../data/rational.js";
import { AccountScreen } from "./account.js";
import { AccountsScreen } from "./accounts.js";
import { BudgetScreen } from "./budget.js";
import { CashFlowScreen } from "./cash-flow.js";
import { DashboardScreen } from "./dashboard.js";
import { GoalsScreen } from "./goals.js";
import { useBehavior } from "./hooks.js";
import { InvestmentsScreen } from "./investments.js";
import { RecurringScreen } from "./recurring.js";
import { useRoute } from "./router.js";
import { SettingsScreen } from "./settings.js";
import { Shell } from "./shell.js";
import { SignInScreen } from "./sign-in.js";
import { TransactionsScreen } from "./transactions.js";

/**
 * Where someone reading the demo can find out how to make it real. The
 * published copy of this app is generated into its own repository; that is
 * where its README lives.
 */
const OWN_PROJECT_GUIDE = "https://github.com/shuaimu/rational#point-it-at-your-own-project";

/**
 * The app runs against its in-browser fake backend whenever it was compiled
 * with no project to talk to — which is what the published demo is. Nothing on
 * screen came from a server and nothing leaves the device, so say so above
 * every screen, signed in or not, rather than letting made-up money read as
 * somebody's money.
 */
function DemoBanner() {
  return (
    <Alert
      variant="warning"
      role="status"
      data-testid="demo-banner"
      className="rounded-none border-x-0 border-t-0 py-2 text-center [&>[data-slot=alert-description]]:justify-items-center"
    >
      <AlertDescription className="block">
        Demo data — this copy talks to no server.{" "}
        <a
          href={OWN_PROJECT_GUIDE}
          target="_blank"
          rel="noreferrer"
          className="font-medium text-foreground underline"
        >
          Point it at a Mako Cloud project
        </a>{" "}
        to make it real.
      </AlertDescription>
    </Alert>
  );
}

export function App({ app }: { app: RationalApp }) {
  return (
    <>
      {app.config.mode === "fake" ? <DemoBanner /> : null}
      <Screens app={app} />
    </>
  );
}

function Screens({ app }: { app: RationalApp }) {
  const state = useBehavior(app.state$);
  const [route] = useRoute();

  if (state.phase === "starting") {
    return (
      <main className="flex min-h-screen items-center justify-center text-muted-foreground">
        <p role="status">Starting…</p>
      </main>
    );
  }
  if (state.phase === "signed_out") {
    return <SignInScreen app={app} state={state} />;
  }
  const session = app.household?.session ?? null;
  const household = state.households.find((candidate) => candidate.id === state.currentHouseholdId);
  const currency = household?.currency ?? "USD";
  return (
    <Shell app={app} state={state} route={route}>
      {route.name === "settings" ? (
        // Settings reads the directory as well as the household, and the
        // members page outlives a household session — a form being filled in
        // must not be thrown away because a session was replaced.
        <ScreenBoundary key={`settings:${state.directory?.generation ?? 0}:${state.generation}`}>
          <SettingsScreen
            app={app}
            state={state}
            session={session}
            route={route}
            currency={currency}
          />
        </ScreenBoundary>
      ) : (
        <ScreenBoundary key={state.generation}>
          {session === null ? (
            <p role="status" data-testid="household-opening">
              {state.memberships.length === 0 ? "Setting up your space…" : "Opening your space…"}
            </p>
          ) : route.name === "transactions" ? (
            <TransactionsScreen
              key={`${state.generation}:transactions`}
              app={app}
              session={session}
              route={route}
              currency={currency}
            />
          ) : route.name === "accounts" ? (
            <AccountsScreen
              key={`${state.generation}:accounts`}
              app={app}
              session={session}
              currency={currency}
            />
          ) : route.name === "account" ? (
            <AccountScreen
              key={`${state.generation}:account:${route.accountId}`}
              app={app}
              session={session}
              currency={currency}
              accountId={route.accountId}
            />
          ) : route.name === "cash-flow" ? (
            <CashFlowScreen
              key={`${state.generation}:cash-flow`}
              app={app}
              session={session}
              route={route}
              currency={currency}
            />
          ) : route.name === "budget" ? (
            <BudgetScreen
              key={`${state.generation}:budget`}
              app={app}
              session={session}
              route={route}
              currency={currency}
              household={household ?? null}
            />
          ) : route.name === "recurring" ? (
            <RecurringScreen
              key={`${state.generation}:recurring`}
              app={app}
              session={session}
              route={route}
              currency={currency}
            />
          ) : route.name === "goals" ? (
            <GoalsScreen
              key={`${state.generation}:goals`}
              app={app}
              session={session}
              currency={currency}
            />
          ) : route.name === "investments" ? (
            <InvestmentsScreen
              key={`${state.generation}:investments`}
              app={app}
              session={session}
              currency={currency}
            />
          ) : (
            <DashboardScreen
              key={`${state.generation}:dashboard`}
              app={app}
              session={session}
              currency={currency}
              household={household ?? null}
            />
          )}
        </ScreenBoundary>
      )}
    </Shell>
  );
}

/** A screen that throws — a query on a database being erased — must not take the shell down. */
class ScreenBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  override state = { failed: false };

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  override render(): ReactNode {
    if (this.state.failed) {
      return (
        <p role="alert" data-testid="screen-error">
          This screen hit a problem and is reloading its data.
        </p>
      );
    }
    return this.props.children;
  }
}
