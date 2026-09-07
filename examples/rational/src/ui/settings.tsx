import {
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  Field,
  Input,
  NativeSelect,
  cn,
} from "@mako-cloud/ui";
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
    <div className="grid gap-6" data-testid="settings-screen">
      {/* The pages are links, not tabs: each has an address of its own, so
          the row is drawn the way the kit draws TabsLine but stays a nav. */}
      <nav
        className="flex w-full flex-wrap items-end gap-4 border-b text-sm"
        aria-label="Settings pages"
      >
        {PAGES.map((entry) => {
          const active = route.page === entry.page;
          return (
            <a
              key={entry.page}
              href={routeHash({ name: "settings", page: entry.page })}
              aria-current={active ? "page" : undefined}
              className={cn(
                "-mb-px inline-flex h-9 items-center border-b-2 border-transparent px-1 pb-2 font-medium text-muted-foreground no-underline transition-colors hover:text-foreground hover:no-underline",
                active && "border-primary text-foreground",
              )}
            >
              {entry.label}
            </a>
          );
        })}
      </nav>
      <div className="min-w-0">
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
      <p className="text-sm text-muted-foreground" role="status" data-testid="household-opening">
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
      <p className="text-sm text-muted-foreground" role="status" data-testid="household-opening">
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
    <section aria-labelledby="household-settings-title" className="grid gap-6">
      <div className="grid gap-1">
        <h1 id="household-settings-title" className="text-2xl">
          Household
        </h1>
        <p className="text-sm text-muted-foreground">
          What this space is called, the currency it thinks in, and how its budget reads.
        </p>
      </div>
      <Card className="max-w-2xl">
        {readOnly ? (
          <CardHeader>
            <CardDescription role="status">
              Only the household's owner can change these settings.
            </CardDescription>
          </CardHeader>
        ) : null}
        <CardContent>
          <form
            className="grid gap-4"
            aria-label="Household settings"
            key={`${household.id}:${household.updated_at}`}
            onSubmit={(event) => void submit(event)}
          >
            <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_8rem]">
              <Field label="Name" htmlFor="settings-name">
                <Input
                  id="settings-name"
                  name="name"
                  required
                  maxLength={200}
                  defaultValue={household.name}
                  disabled={readOnly}
                />
              </Field>
              <Field label="Currency" htmlFor="settings-currency">
                <Input
                  id="settings-currency"
                  name="currency"
                  required
                  maxLength={3}
                  pattern="[A-Za-z]{3}"
                  className="uppercase"
                  defaultValue={household.currency}
                  disabled={readOnly}
                />
              </Field>
            </div>
            <Field
              label="Budget mode"
              htmlFor="settings-budget-mode"
              hint="The budget mode changes how the budget page presents the same budgets, never the budgets themselves."
            >
              <NativeSelect
                id="settings-budget-mode"
                name="budget_mode"
                defaultValue={(household.budget_mode ?? "category") satisfies BudgetMode}
                disabled={readOnly}
                data-testid="budget-mode"
              >
                <option value="category">By category — a number per category and group</option>
                <option value="flex">Flex — fixed, non-monthly, and one flexible number</option>
              </NativeSelect>
            </Field>
            {error === null ? null : (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
            {saved ? (
              <p
                className="text-sm text-muted-foreground"
                role="status"
                data-testid="household-saved"
              >
                Saved.
              </p>
            ) : null}
            <div className="flex gap-2">
              <Button type="submit" disabled={readOnly || busy}>
                Save settings
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
    </section>
  );
}
