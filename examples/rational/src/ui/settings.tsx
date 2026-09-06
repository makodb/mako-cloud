import { type FormEvent, useState } from "react";

import type { AppState, RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { BudgetMode, HouseholdCollectionId } from "../model/types.js";
import { AlertsScreen } from "./alerts.js";
import { CategoriesScreen } from "./categories.js";
import { ConnectionsScreen } from "./connections.js";
import { DataScreen } from "./data.js";
import { HouseholdScreen } from "./household.js";
import { ImportScreen } from "./import.js";
import { MerchantsScreen } from "./merchants.js";
import { type Route, routeHash, type SettingsPage } from "./router.js";
import { RulesScreen } from "./rules.js";
import { TagsScreen } from "./tags.js";
import "./styles/settings.css";

/**
 * Everything that configures the household rather than shows its money, in
 * one place with a second-level navigation — Monarch's Settings. Each page is
 * its own screen; this is the frame that puts them side by side.
 */
const PAGES: ReadonlyArray<{ readonly page: SettingsPage; readonly label: string }> = [
  { page: "household", label: "Household" },
  { page: "members", label: "Members" },
  { page: "categories", label: "Categories" },
  { page: "tags", label: "Tags" },
  { page: "merchants", label: "Merchants" },
  { page: "rules", label: "Rules" },
  { page: "connections", label: "Connections" },
  { page: "import", label: "Import" },
  { page: "notifications", label: "Notifications" },
  { page: "data", label: "Data" },
];

export function SettingsScreen({
  app,
  state,
  session,
  route,
  currency,
}: {
  app: RationalApp;
  state: AppState;
  session: ScopeSession<HouseholdCollectionId> | null;
  route: Extract<Route, { name: "settings" }>;
  currency: string;
}) {
  return (
    <div className="settings" data-testid="settings-screen">
      <nav className="settings-nav" aria-label="Settings pages">
        {PAGES.map((entry) => (
          <a
            key={entry.page}
            href={routeHash({ name: "settings", page: entry.page })}
            aria-current={route.page === entry.page ? "page" : undefined}
          >
            {entry.label}
          </a>
        ))}
      </nav>
      <div className="settings-page">
        <SettingsPageBody
          app={app}
          state={state}
          session={session}
          page={route.page}
          currency={currency}
        />
      </div>
    </div>
  );
}

function SettingsPageBody({
  app,
  state,
  session,
  page,
  currency,
}: {
  app: RationalApp;
  state: AppState;
  session: ScopeSession<HouseholdCollectionId> | null;
  page: SettingsPage;
  currency: string;
}) {
  if (page === "members") return <HouseholdScreen app={app} state={state} />;
  if (page === "household") return <HouseholdSettings app={app} state={state} />;
  if (session === null) {
    return (
      <p role="status" data-testid="household-opening">
        {state.memberships.length === 0 ? "Setting up your space…" : "Opening your space…"}
      </p>
    );
  }
  switch (page) {
    case "categories":
      return <CategoriesScreen app={app} session={session} />;
    case "tags":
      return <TagsScreen app={app} session={session} />;
    case "merchants":
      return <MerchantsScreen app={app} session={session} currency={currency} />;
    case "rules":
      return <RulesScreen app={app} session={session} currency={currency} />;
    case "connections":
      return <ConnectionsScreen app={app} session={session} />;
    case "import":
      return <ImportScreen app={app} session={session} currency={currency} />;
    case "notifications":
      return <AlertsScreen app={app} session={session} currency={currency} />;
    default:
      return <DataScreen app={app} session={session} currency={currency} />;
  }
}

/**
 * The household's own settings: what it is called, the currency it thinks in,
 * and how its budget page presents the same budgets. Only the owner may change
 * them — the policy on the households collection says so, and the form says
 * so too rather than letting an editor find out from a refusal.
 */
function HouseholdSettings({ app, state }: { app: RationalApp; state: AppState }) {
  const household =
    state.households.find((candidate) => candidate.id === state.currentHouseholdId) ?? null;
  const role = app.roleIn(state.currentHouseholdId);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

  if (household === null) {
    return (
      <p role="status" data-testid="household-opening">
        Opening your space…
      </p>
    );
  }
  const readOnly = role !== "owner";

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setError(null);
    setSaved(false);
    setBusy(true);
    try {
      await app.updateHousehold({
        name: String(data.get("name") ?? ""),
        currency: String(data.get("currency") ?? "").toUpperCase(),
        budget_mode: data.get("budget_mode") === "flex" ? "flex" : "category",
      });
      setSaved(true);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "The settings could not be saved.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="household-settings-title">
      <div className="heading">
        <h1 id="household-settings-title">Household</h1>
      </div>
      {readOnly ? (
        <p className="hint" role="status">
          Only the household's owner can change these settings.
        </p>
      ) : null}
      <form
        className="editor"
        aria-label="Household settings"
        key={`${household.id}:${household.updated_at}`}
        onSubmit={(event) => void submit(event)}
      >
        <div className="grid">
          <label>
            Name
            <input
              name="name"
              required
              maxLength={200}
              defaultValue={household.name}
              disabled={readOnly}
            />
          </label>
          <label>
            Currency
            <input
              name="currency"
              required
              maxLength={3}
              pattern="[A-Za-z]{3}"
              defaultValue={household.currency}
              disabled={readOnly}
            />
          </label>
          <label>
            Budget mode
            <select
              name="budget_mode"
              defaultValue={(household.budget_mode ?? "category") satisfies BudgetMode}
              disabled={readOnly}
              data-testid="budget-mode"
            >
              <option value="category">By category — a number per category and group</option>
              <option value="flex">Flex — fixed, non-monthly, and one flexible number</option>
            </select>
          </label>
        </div>
        <p className="hint">
          The budget mode changes how the budget page presents the same budgets, never the budgets
          themselves.
        </p>
        {error === null ? null : (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        {saved ? (
          <p className="hint" role="status" data-testid="household-saved">
            Saved.
          </p>
        ) : null}
        <div className="actions">
          <button type="submit" disabled={readOnly || busy}>
            Save settings
          </button>
        </div>
      </form>
    </section>
  );
}
