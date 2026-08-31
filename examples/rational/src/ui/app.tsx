import { Component, type ReactNode } from "react";

import type { RationalApp } from "../data/rational.js";
import { AccountsScreen } from "./accounts.js";
import { AlertsScreen } from "./alerts.js";
import { BudgetsScreen } from "./budgets.js";
import { CategoriesScreen } from "./categories.js";
import { ConnectionsScreen } from "./connections.js";
import { useBehavior } from "./hooks.js";
import { HouseholdScreen } from "./household.js";
import { ImportScreen } from "./import.js";
import { PlanScreen } from "./plan.js";
import { ReportsScreen } from "./reports.js";
import { useRoute } from "./router.js";
import { RulesScreen } from "./rules.js";
import { Shell } from "./shell.js";
import { SignInScreen } from "./sign-in.js";
import { TagsScreen } from "./tags.js";
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
    <div className="banner demo" role="status" data-testid="demo-banner">
      Demo data — this copy talks to no server.{" "}
      <a href={OWN_PROJECT_GUIDE} target="_blank" rel="noreferrer">
        Point it at a Mako Cloud project
      </a>{" "}
      to make it real.
    </div>
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
      <main className="centered">
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
      {route.name === "household" ? (
        // Households and members come from the directory, not from the open
        // household, so this screen outlives a household session — and a form
        // being filled in must not be thrown away because one was replaced.
        <ScreenBoundary key={`directory:${state.directory?.generation ?? 0}`}>
          <HouseholdScreen app={app} state={state} />
        </ScreenBoundary>
      ) : (
        <ScreenBoundary key={state.generation}>
          {session === null ? (
            <p role="status" data-testid="household-opening">
              {state.memberships.length === 0
                ? "You are not a member of any household yet. Create or accept one from the Household screen."
                : "Opening household…"}
            </p>
          ) : route.name === "transactions" ? (
            <TransactionsScreen
              key={`${state.generation}:transactions`}
              app={app}
              session={session}
              route={route}
              currency={currency}
            />
          ) : route.name === "budgets" ? (
            <BudgetsScreen
              key={`${state.generation}:budgets`}
              app={app}
              session={session}
              route={route}
              currency={currency}
            />
          ) : route.name === "plan" ? (
            <PlanScreen
              key={`${state.generation}:plan`}
              app={app}
              session={session}
              currency={currency}
            />
          ) : route.name === "connections" ? (
            <ConnectionsScreen
              key={`${state.generation}:connections`}
              app={app}
              session={session}
            />
          ) : route.name === "import" ? (
            <ImportScreen
              key={`${state.generation}:import`}
              app={app}
              session={session}
              currency={currency}
            />
          ) : route.name === "rules" ? (
            <RulesScreen
              key={`${state.generation}:rules`}
              app={app}
              session={session}
              currency={currency}
            />
          ) : route.name === "alerts" ? (
            <AlertsScreen
              key={`${state.generation}:alerts`}
              app={app}
              session={session}
              currency={currency}
            />
          ) : route.name === "reports" ? (
            <ReportsScreen
              key={`${state.generation}:reports`}
              app={app}
              session={session}
              route={route}
            />
          ) : route.name === "categories" ? (
            <CategoriesScreen key={`${state.generation}:categories`} app={app} session={session} />
          ) : route.name === "tags" ? (
            <TagsScreen key={`${state.generation}:tags`} app={app} session={session} />
          ) : (
            <AccountsScreen
              key={`${state.generation}:accounts`}
              app={app}
              session={session}
              currency={currency}
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
