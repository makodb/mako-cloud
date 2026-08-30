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
 * Alerts: what the household asked to be told, and what it has been told.
 *
 * Nothing on this screen decides an alert. The deciding happens on the server
 * -- in the nightly job, and after each institution sync -- because a device
 * that is closed would never fire one, and the charge worth knowing about is
 * usually the one that arrived while nobody was looking. What arrives here is
 * a document like any other, through the same replication as everything else.
 *
 * A person can mark an alert read. Nothing deletes one: the history is the
 * point, and a household that fired an alert and then lost the record of it
 * has been told nothing.
 */
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
  const history = firedAlertHistory(documents);
  const accountName = (id: string | undefined) =>
    id === undefined ? "" : (accounts.find((account) => account.id === id)?.name ?? id);

  const save = async (kind: AlertKind, event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      await app.writes?.saveAlertSetting({
        alert_kind: kind,
        threshold: parseAmount(String(data.get("threshold") ?? "0"), currency),
        enabled: data.get("enabled") === "on",
      });
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the setting could not be saved");
    }
  };

  return (
    <section aria-labelledby="alerts-title" data-testid="alerts-screen">
      <div className="section-heading">
        <div>
          <h2 id="alerts-title">Alerts</h2>
          <p className="muted">
            Decided while nobody is watching — overnight, and after each sync — so a closed laptop
            does not mean a missed alert.
          </p>
        </div>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}

      <h3>What to tell me about</h3>
      {ALERT_KINDS.map((kind) => {
        const setting = settingFor(documents, kind);
        return (
          <form
            key={kind}
            className="filters"
            aria-label={ALERT_LABELS[kind]}
            data-testid={`alert-setting-${kind}`}
            onSubmit={(event) => void save(kind, event)}
          >
            <strong>{ALERT_LABELS[kind]}</strong>
            <label>
              {ALERT_THRESHOLD_LABELS[kind]}
              <input
                name="threshold"
                defaultValue={
                  setting === null ? "" : amountToText(setting.threshold ?? 0, currency)
                }
                key={`${kind}-${setting?.updated_at ?? "new"}`}
                inputMode="decimal"
                placeholder="0.00"
              />
            </label>
            <label className="chip-option">
              <input
                type="checkbox"
                name="enabled"
                defaultChecked={setting === null ? true : setting.enabled !== false}
                key={`${kind}-enabled-${setting?.updated_at ?? "new"}`}
              />
              Enabled
            </label>
            <button type="submit">Save</button>
          </form>
        );
      })}

      <h3>What I have been told</h3>
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
              <th scope="col">Amount</th>
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
                  {alert.amount === undefined
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
