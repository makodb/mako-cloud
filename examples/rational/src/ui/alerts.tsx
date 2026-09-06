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
import "./styles/settings-pages.css";

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
    <section aria-labelledby="alerts-title" data-testid="alerts-screen">
      <div className="heading">
        <h1 id="alerts-title">Notifications</h1>
      </div>
      <p className="hint">
        Nothing on this device decides a notification. The server does — the nightly job reads the
        household once a night, and each connection's sync reports as it finishes — so a closed
        laptop does not mean a missed alert. What is decided arrives here as a document, the same
        way a transaction does.
      </p>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}

      <h3>What to tell me about</h3>
      <div className="alert-settings">
        {ALERT_KINDS.map((kind) => {
          const setting = settingFor(documents, kind);
          const thresholdKind = THRESHOLD_KINDS[kind];
          const version = setting?.updated_at ?? "new";
          return (
            <form
              key={kind}
              className="alert-setting"
              aria-label={ALERT_LABELS[kind]}
              data-testid={`alert-setting-${kind}`}
              onSubmit={(event) => void save(kind, event)}
            >
              <strong>{ALERT_LABELS[kind]}</strong>
              {thresholdKind === "amount" ? (
                <label className="threshold">
                  {ALERT_THRESHOLD_LABELS[kind]}
                  <input
                    name="threshold"
                    aria-label={ALERT_THRESHOLD_LABELS[kind]}
                    defaultValue={
                      setting === null ? "" : amountToText(setting.threshold ?? 0, currency)
                    }
                    key={`${kind}-${version}`}
                    inputMode="decimal"
                    placeholder="0.00"
                  />
                </label>
              ) : thresholdKind === "days" ? (
                <label className="threshold">
                  {ALERT_THRESHOLD_LABELS[kind]}
                  <input
                    name="threshold"
                    type="number"
                    aria-label={ALERT_THRESHOLD_LABELS[kind]}
                    min={0}
                    max={365}
                    step={1}
                    defaultValue={setting?.threshold ?? DEFAULT_BILL_DUE_DAYS}
                    key={`${kind}-${version}`}
                  />
                </label>
              ) : (
                <p className="hint no-threshold">{ALERT_THRESHOLD_LABELS[kind]}</p>
              )}
              <label className="chip-option">
                <input
                  type="checkbox"
                  name="enabled"
                  defaultChecked={setting === null ? true : setting.enabled !== false}
                  key={`${kind}-enabled-${version}`}
                />
                Enabled
              </label>
              <button type="submit">Save</button>
            </form>
          );
        })}
      </div>

      <div className="section-heading">
        <h3>What I have been told</h3>
        {history.length === 0 ? null : (
          <button
            type="button"
            className="secondary"
            disabled={marking || unread.length === 0}
            data-testid="mark-all-read"
            onClick={() => void markAllRead()}
          >
            Mark all read
          </button>
        )}
      </div>
      {history.length === 0 ? (
        <p className="muted" data-testid="alerts-empty">
          Nothing yet. An alert appears here the moment the server decides one, whether or not this
          tab was open when it did.
        </p>
      ) : (
        <table className="data-table" aria-label="Alert history">
          <thead>
            <tr>
              <th scope="col">When</th>
              <th scope="col">What</th>
              <th scope="col" className="amount">
                Amount
              </th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {history.map((alert) => (
              <tr
                key={alert.id}
                data-testid={`alert-${alert.id}`}
                data-read={alert.read === true ? "yes" : "no"}
                className={alert.read === true ? "muted" : undefined}
              >
                <td>
                  {alert.fired_at === undefined
                    ? "—"
                    : new Date(alert.fired_at).toISOString().slice(0, 10)}
                </td>
                <td>
                  <span data-testid="alert-kind">{ALERT_LABELS[alert.alert_kind]}</span>{" "}
                  <small className="muted" data-testid="alert-message">
                    {alert.message}
                    {alert.account_id === undefined ? "" : ` — ${accountName(alert.account_id)}`}
                  </small>
                </td>
                <td className="amount">
                  {alert.amount === undefined || alert.amount === 0
                    ? "—"
                    : formatMinorUnits(alert.amount, alert.currency ?? currency)}
                </td>
                <td className="actions">
                  <button
                    type="button"
                    className="link"
                    onClick={() => {
                      void app.writes?.markAlertRead(alert.id, alert.read !== true);
                    }}
                  >
                    {alert.read === true ? "Mark unread" : "Mark read"}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
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
