import { type FormEvent, useEffect, useState } from "react";

import { openPlaidLink } from "../data/plaid.js";
import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { ConnectionDocument, HouseholdCollectionId } from "../model/types.js";
import { settingFor } from "../selectors/alerts.js";
import { useQuery } from "./hooks.js";
import { routeHash } from "./router.js";
import "./styles/settings-pages.css";

/**
 * Connected accounts and what the scheduled sync last did with them.
 *
 * The connection is written from here; from then on the `institution-sync`
 * function owns its last sync time and outcome. Nothing on this screen syncs
 * anything: the schedule does that with nobody signed in, which is the point
 * of it. What the screen shows is whether that is working -- and, when it is
 * not, whether the household has asked to be told.
 */

/** A connection's status in words; a document from before statuses existed reads as connected. */
type ConnectionStatus = NonNullable<ConnectionDocument["status"]>;

const STATUS_LABELS: Readonly<Record<ConnectionStatus, string>> = {
  connected: "connected",
  error: "error",
  disconnected: "disconnected",
};

export function ConnectionsScreen({
  app,
  session,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
}) {
  const connections = useQuery(session.collection("connections")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const alerts = useQuery(session.collection("alerts")?.find() ?? null);
  const [problem, setProblem] = useState<string | null>(null);
  // Whether the deployment can talk to Plaid at all. Asked once, of the
  // function itself: a copy deployed without Plaid credentials answers no and
  // this screen simply never offers the option.
  const [plaidReady, setPlaidReady] = useState(false);
  const [plaidBusy, setPlaidBusy] = useState(false);
  useEffect(() => {
    let cancelled = false;
    app.plaid
      ?.configured()
      .then((configured) => {
        if (!cancelled) setPlaidReady(configured);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [app.plaid]);
  const institutions = connections.filter(
    (entry) => entry.kind === "institution" || entry.kind === "plaid",
  );
  const imports = connections.filter((entry) => entry.kind === "import");
  const failing = institutions.filter((entry) => entry.status === "error");
  const syncAlert = settingFor(alerts, "sync_error");
  const accountName = (id: string | undefined) =>
    id === undefined ? "—" : (accounts.find((account) => account.id === id)?.name ?? id);

  const connectPlaid = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const accountId = String(new FormData(form).get("account_id") ?? "");
    const client = app.plaid;
    const householdId = app.state.currentHouseholdId;
    if (client === null || householdId === null || accountId === "") return;
    setPlaidBusy(true);
    try {
      // Token from the function, widget from Plaid, public token straight
      // back to the function. No token that outlives this handler ever
      // touches the browser.
      const linked = await openPlaidLink(await client.linkToken());
      await client.exchange({
        publicToken: linked.publicToken,
        householdId,
        accountId,
        institution: linked.institution,
      });
      form.reset();
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the institution could not be linked");
    } finally {
      setPlaidBusy(false);
    }
  };

  const connect = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      await app.writes?.connectInstitution({
        account_id: String(data.get("account_id") ?? ""),
        institution: String(data.get("institution") ?? ""),
        external_id: String(data.get("external_id") ?? ""),
      });
      form.reset();
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the connection could not be saved");
    }
  };

  return (
    <section aria-labelledby="connections-title" data-testid="connections-screen">
      <div className="heading">
        <h1 id="connections-title">Connections</h1>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}

      <h3>Connected accounts</h3>
      {institutions.length === 0 ? (
        <p className="muted">No connected accounts. A connected account syncs on its own.</p>
      ) : (
        <table className="data-table" aria-label="Connected accounts">
          <thead>
            <tr>
              <th scope="col">Institution</th>
              <th scope="col">Account</th>
              <th scope="col">Status</th>
              <th scope="col">Last sync</th>
              <th scope="col">Outcome</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {institutions.map((connection) => {
              const status: ConnectionStatus = connection.status ?? "connected";
              return (
                <tr
                  key={connection.id}
                  data-testid={`connection-${connection.id}`}
                  data-status={status}
                >
                  <th scope="row">{connection.institution}</th>
                  <td>{accountName(connection.account_id)}</td>
                  <td>
                    <span className={`chip status-${status}`} data-testid="status">
                      {STATUS_LABELS[status]}
                    </span>
                  </td>
                  <td data-testid="last-sync">
                    {connection.last_sync_at === undefined
                      ? "never"
                      : new Date(connection.last_sync_at).toLocaleString()}
                  </td>
                  <td data-testid="outcome">{connection.last_sync_outcome ?? "—"}</td>
                  <td className="actions">
                    <button
                      type="button"
                      className="link"
                      onClick={() =>
                        void app.writes?.setConnectionStatus(
                          connection.id,
                          status === "connected" ? "disconnected" : "connected",
                        )
                      }
                    >
                      {status === "connected" ? "Disconnect" : "Reconnect"}
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {/* A failing sync is decided server-side and arrives as a notification
          -- but only if the household asked for that kind. Say where that
          stands rather than letting a broken connection go quiet. */}
      <p className="hint" data-testid="sync-error-hint" data-alert={syncAlertState(syncAlert)}>
        {failing.length === 0
          ? "When a connection's sync fails, the sync itself raises a notification naming the institution; a connection that keeps failing stays one notification until it is fixed. "
          : `${failing.length === 1 ? "One connection is" : `${failing.length} connections are`} failing to sync; the sync raised a notification for each. `}
        {syncAlert === null
          ? "That notification is not set up yet — save “A connection that stopped syncing” under "
          : syncAlert.enabled === false
            ? "That notification is turned off — turn it on under "
            : "That notification is on; change it under "}
        <a href={routeHash({ name: "settings", page: "notifications" })}>Notifications</a>.
      </p>

      <h3>Imports</h3>
      {imports.length === 0 ? (
        <p className="muted">No imports yet.</p>
      ) : (
        <ul aria-label="Imports">
          {imports
            .slice()
            .sort((left, right) => (right.imported_at ?? 0) - (left.imported_at ?? 0))
            .map((batch) => (
              <li key={batch.id} data-testid={`import-${batch.id}`}>
                {batch.filename} into {accountName(batch.account_id)} — {batch.created_count ?? 0}{" "}
                of {batch.row_count ?? 0} rows, {batch.duplicate_count ?? 0} already here
              </li>
            ))}
        </ul>
      )}
      <p className="hint">
        A CSV statement is imported under{" "}
        <a href={routeHash({ name: "settings", page: "import" })}>Import</a>; what arrives waits in
        Needs review until somebody looks at it.
      </p>

      {plaidReady ? (
        <form
          className="editor"
          aria-label="Connect through Plaid"
          data-testid="plaid-connect"
          onSubmit={(event) => void connectPlaid(event)}
        >
          <h3>Connect through Plaid</h3>
          <p className="muted">
            Link a real institution in Plaid&apos;s sandbox. The connection syncs on the same
            fifteen-minute schedule as everything else; no credential ever reaches this browser.
          </p>
          <div className="grid">
            <label>
              Account
              <select name="account_id" defaultValue="" required>
                <option value="">Choose an account</option>
                {accounts.map((account) => (
                  <option key={account.id} value={account.id}>
                    {account.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="actions">
            <button type="submit" disabled={plaidBusy}>
              {plaidBusy ? "Linking…" : "Connect through Plaid"}
            </button>
          </div>
        </form>
      ) : null}

      <form
        className="editor"
        aria-label="Connect an account"
        onSubmit={(event) => void connect(event)}
      >
        <h3>Connect an account</h3>
        <p className="muted">
          The simulated institution: it hands the scheduled sync a few transactions for whatever
          account id it is asked about.
        </p>
        <div className="grid">
          <label>
            Account
            <select name="account_id" defaultValue="" required>
              <option value="">Choose an account</option>
              {accounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            Institution
            <input name="institution" required maxLength={200} defaultValue="Simulated Bank" />
          </label>
          <label>
            Their account id
            <input name="external_id" required maxLength={64} placeholder="acct-1" />
          </label>
        </div>
        <div className="actions">
          <button type="submit">Connect</button>
        </div>
      </form>
    </section>
  );
}

/** Where the household stands on being told about a failing sync, for the hint's attribute. */
function syncAlertState(setting: { readonly enabled?: boolean } | null): "unset" | "off" | "on" {
  if (setting === null) return "unset";
  return setting.enabled === false ? "off" : "on";
}
