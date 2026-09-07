import {
  Alert,
  AlertDescription,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  Checkbox,
  EmptyState,
  Field,
  Input,
  Label,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { Bell, CircleAlert } from "lucide-react";
import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId } from "../model/types.js";
import {
  ALERT_KINDS,
  ALERT_LABELS,
  ALERT_THRESHOLD_LABELS,
  type AlertKind,
  firedAlertHistory,
  settingFor,
} from "../selectors/alerts.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { useQuery } from "./hooks.js";

/**
 * Notifications: what the household asked to be told, and what it has been told.
 *
 * Nothing on this screen decides an alert. The deciding happens on the server
 * -- in the nightly job, and after each institution sync -- because a device
 * that is closed would never fire one, and the charge worth knowing about is
 * usually the one that arrived while nobody was looking. What arrives here is
 * a document like any other, through the same replication as everything else.
 *
 * A person can mark an alert read, one at a time or all at once. Nothing
 * deletes one: the history is the point, and a household that fired an alert
 * and then lost the record of it has been told nothing.
 */

/** What a kind's threshold is: money, a count of days, or nothing to choose. */
type ThresholdKind = "amount" | "days" | "none";

const THRESHOLD_KINDS: Readonly<Record<AlertKind, ThresholdKind>> = {
  large_transaction: "amount",
  budget_exceeded: "amount",
  low_balance: "amount",
  bill_due: "days",
  goal_reached: "none",
  sync_error: "none",
};

/** A bill is announced a few days ahead unless the household says otherwise. */
const DEFAULT_BILL_DUE_DAYS = 3;

export function AlertsScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const documents = useQuery(session.collection("alerts")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const [problem, setProblem] = useState<string | null>(null);
  const [marking, setMarking] = useState(false);
  const history = firedAlertHistory(documents);
  const unread = history.filter((alert) => alert.read !== true);
  const accountName = (id: string | undefined) =>
    id === undefined ? "" : (accounts.find((account) => account.id === id)?.name ?? id);

  const save = async (kind: AlertKind, event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      await app.writes?.saveAlertSetting({
        alert_kind: kind,
        threshold: readThreshold(kind, String(data.get("threshold") ?? ""), currency),
        enabled: data.get("enabled") === "on",
      });
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the setting could not be saved");
    }
  };

  // One write per alert, each through the same method a single "Mark read"
  // uses: the history stays a set of documents, and a device that is offline
  // queues them like any other change.
  const markAllRead = async () => {
    setMarking(true);
    try {
      for (const alert of unread) {
        await app.writes?.markAlertRead(alert.id, true);
      }
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the alerts could not be marked read");
    } finally {
      setMarking(false);
    }
  };

  return (
    <section aria-labelledby="alerts-title" data-testid="alerts-screen" className="grid gap-6">
      <div className="grid gap-1">
        <h1 id="alerts-title" className="text-2xl">
          Notifications
        </h1>
        <p className="text-sm text-muted-foreground">
          What the household asked to be told about, and what it has been told.
        </p>
      </div>
      {problem === null ? null : (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription className="block">{problem}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <h3 className="text-base font-semibold leading-none">What to tell me about</h3>
          <CardDescription className="max-w-prose">
            Nothing on this device decides a notification. The server does — the nightly job reads
            the household once a night, and each connection's sync reports as it finishes — so a
            closed laptop does not mean a missed alert. What is decided arrives here as a document,
            the same way a transaction does.
          </CardDescription>
        </CardHeader>
        <CardContent className="divide-y">
          {ALERT_KINDS.map((kind) => {
            const setting = settingFor(documents, kind);
            const thresholdKind = THRESHOLD_KINDS[kind];
            const version = setting?.updated_at ?? "new";
            const thresholdId = `alert-threshold-${kind}`;
            const enabledId = `alert-enabled-${kind}`;
            return (
              <form
                key={kind}
                className="grid items-end gap-4 py-4 first:pt-0 last:pb-0 md:grid-cols-[minmax(11rem,1.2fr)_minmax(12rem,2fr)_auto_auto]"
                aria-label={ALERT_LABELS[kind]}
                data-testid={`alert-setting-${kind}`}
                onSubmit={(event) => void save(kind, event)}
              >
                <strong className="self-center text-sm font-semibold">{ALERT_LABELS[kind]}</strong>
                {thresholdKind === "amount" ? (
                  <Field label={ALERT_THRESHOLD_LABELS[kind]} htmlFor={thresholdId}>
                    <Input
                      id={thresholdId}
                      name="threshold"
                      aria-label={ALERT_THRESHOLD_LABELS[kind]}
                      defaultValue={
                        setting === null ? "" : amountToText(setting.threshold ?? 0, currency)
                      }
                      key={`${kind}-${version}`}
                      inputMode="decimal"
                      placeholder="0.00"
                      className="max-w-48 money"
                    />
                  </Field>
                ) : thresholdKind === "days" ? (
                  <Field label={ALERT_THRESHOLD_LABELS[kind]} htmlFor={thresholdId}>
                    <Input
                      id={thresholdId}
                      name="threshold"
                      type="number"
                      aria-label={ALERT_THRESHOLD_LABELS[kind]}
                      min={0}
                      max={365}
                      step={1}
                      defaultValue={setting?.threshold ?? DEFAULT_BILL_DUE_DAYS}
                      key={`${kind}-${version}`}
                      className="max-w-48"
                    />
                  </Field>
                ) : (
                  <p className="self-center text-sm text-muted-foreground">
                    {ALERT_THRESHOLD_LABELS[kind]}
                  </p>
                )}
                <div
                  className={cn(
                    "flex items-center gap-2",
                    thresholdKind === "none" ? "self-center" : "self-end pb-2.5",
                  )}
                >
                  <Checkbox
                    id={enabledId}
                    name="enabled"
                    defaultChecked={setting === null ? true : setting.enabled !== false}
                    key={`${kind}-enabled-${version}`}
                  />
                  <Label htmlFor={enabledId}>Enabled</Label>
                </div>
                <Button type="submit" variant="secondary" className="justify-self-start">
                  Save
                </Button>
              </form>
            );
          })}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <h3 className="text-base font-semibold leading-none">What I have been told</h3>
          {history.length === 0 ? null : (
            <CardAction>
              <Button
                variant="outline"
                size="sm"
                disabled={marking || unread.length === 0}
                data-testid="mark-all-read"
                onClick={() => void markAllRead()}
              >
                Mark all read
              </Button>
            </CardAction>
          )}
        </CardHeader>
        <CardContent>
          {history.length === 0 ? (
            <div data-testid="alerts-empty">
              <EmptyState
                icon={<Bell />}
                title="Nothing yet."
                description="An alert appears here the moment the server decides one, whether or not this tab was open when it did."
              />
            </div>
          ) : (
            <Table aria-label="Alert history">
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">When</TableHead>
                  <TableHead scope="col">What</TableHead>
                  <TableHead scope="col" className="text-right">
                    Amount
                  </TableHead>
                  <TableHead scope="col">
                    <span className="sr-only">Actions</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {history.map((alert) => (
                  <TableRow
                    key={alert.id}
                    data-testid={`alert-${alert.id}`}
                    data-read={alert.read === true ? "yes" : "no"}
                    className={alert.read === true ? "text-muted-foreground" : undefined}
                  >
                    <TableCell className="tabular-nums">
                      {alert.fired_at === undefined
                        ? "—"
                        : new Date(alert.fired_at).toISOString().slice(0, 10)}
                    </TableCell>
                    <TableCell className="whitespace-normal">
                      <span data-testid="alert-kind" className="font-medium">
                        {ALERT_LABELS[alert.alert_kind]}
                      </span>{" "}
                      <small className="text-muted-foreground" data-testid="alert-message">
                        {alert.message}
                        {alert.account_id === undefined
                          ? ""
                          : ` — ${accountName(alert.account_id)}`}
                      </small>
                    </TableCell>
                    <TableCell className="money">
                      {alert.amount === undefined || alert.amount === 0
                        ? "—"
                        : formatMinorUnits(alert.amount, alert.currency ?? currency)}
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="link"
                        size="sm"
                        className="h-auto p-0"
                        onClick={() => {
                          void app.writes?.markAlertRead(alert.id, alert.read !== true);
                        }}
                      >
                        {alert.read === true ? "Mark unread" : "Mark read"}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </section>
  );
}

/**
 * The threshold as the write wants it: minor units for the money alerts, a
 * whole number of days for a bill, and zero where there is nothing to choose.
 * A blank field is refused rather than read as zero: zero would mean "tell me
 * about every charge" or "tell me on the day", and neither is what an empty
 * box says.
 */
function readThreshold(kind: AlertKind, text: string, currency: string): number {
  const thresholdKind = THRESHOLD_KINDS[kind];
  if (thresholdKind === "none") return 0;
  const trimmed = text.trim();
  if (thresholdKind === "days") {
    if (trimmed === "") throw new RangeError("say how many days ahead to be told");
    return Number(trimmed);
  }
  if (trimmed === "") throw new RangeError("choose an amount first");
  return parseAmount(trimmed, currency);
}
