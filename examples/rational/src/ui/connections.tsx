import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  EmptyState,
  Field,
  Input,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { CircleAlert, FileSpreadsheet, Landmark, Plug } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { openPlaidLink } from "../data/plaid.js";
import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { ConnectionDocument, HouseholdCollectionId } from "../model/types.js";
import { settingFor } from "../selectors/alerts.js";
import { useQuery } from "./hooks.js";
import { routeHash } from "./router.js";

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

/** The one place a connection earns a colour: green while it lands, red once it failed. */
const STATUS_VARIANTS: Readonly<
  Record<ConnectionStatus, "positive" | "destructive" | "secondary">
> = {
  connected: "positive",
  error: "destructive",
  disconnected: "secondary",
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
    <section
      aria-labelledby="connections-title"
      data-testid="connections-screen"
      className="grid gap-6"
    >
      <div className="grid gap-1">
        <h1 id="connections-title" className="text-2xl">
          Connections
        </h1>
        <p className="text-sm text-muted-foreground">
          The institutions this household is linked to, and what the scheduled sync last did with
          them.
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
          <h3 className="text-base font-semibold leading-none">Connected accounts</h3>
        </CardHeader>
        <CardContent className="grid gap-4">
          {institutions.length === 0 ? (
            <EmptyState
              icon={<Plug />}
              title="No connected accounts."
              description="A connected account syncs on its own."
            />
          ) : (
            <Table aria-label="Connected accounts">
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">Institution</TableHead>
                  <TableHead scope="col">Account</TableHead>
                  <TableHead scope="col">Status</TableHead>
                  <TableHead scope="col">Last sync</TableHead>
                  <TableHead scope="col">Outcome</TableHead>
                  <TableHead scope="col">
                    <span className="sr-only">Actions</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {institutions.map((connection) => {
                  const status: ConnectionStatus = connection.status ?? "connected";
                  return (
                    <TableRow
                      key={connection.id}
                      data-testid={`connection-${connection.id}`}
                      data-status={status}
                    >
                      <TableHead scope="row" className="h-auto p-2 text-foreground">
                        {connection.institution}
                      </TableHead>
                      <TableCell>{accountName(connection.account_id)}</TableCell>
                      <TableCell>
                        <Badge variant={STATUS_VARIANTS[status]} data-testid="status">
                          {STATUS_LABELS[status]}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-muted-foreground" data-testid="last-sync">
                        {connection.last_sync_at === undefined
                          ? "never"
                          : new Date(connection.last_sync_at).toLocaleString()}
                      </TableCell>
                      <TableCell data-testid="outcome">
                        {connection.last_sync_outcome ?? "—"}
                      </TableCell>
                      <TableCell className="text-right">
                        <Button
                          variant="link"
                          size="sm"
                          className="h-auto p-0"
                          onClick={() =>
                            void app.writes?.setConnectionStatus(
                              connection.id,
                              status === "connected" ? "disconnected" : "connected",
                            )
                          }
                        >
                          {status === "connected" ? "Disconnect" : "Reconnect"}
                        </Button>
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
          {/* A failing sync is decided server-side and arrives as a notification
              -- but only if the household asked for that kind. Say where that
              stands rather than letting a broken connection go quiet. */}
          <p
            className="max-w-prose text-sm text-muted-foreground"
            data-testid="sync-error-hint"
            data-alert={syncAlertState(syncAlert)}
          >
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
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <h3 className="text-base font-semibold leading-none">Imports</h3>
        </CardHeader>
        <CardContent className="grid gap-4">
          {imports.length === 0 ? (
            <p className="text-sm text-muted-foreground">No imports yet.</p>
          ) : (
            <ul className="m-0 grid list-none gap-2 p-0 text-sm" aria-label="Imports">
              {imports
                .slice()
                .sort((left, right) => (right.imported_at ?? 0) - (left.imported_at ?? 0))
                .map((batch) => (
                  <li key={batch.id} data-testid={`import-${batch.id}`} className="flex gap-2">
                    <FileSpreadsheet
                      aria-hidden="true"
                      className="mt-0.5 size-4 shrink-0 text-muted-foreground"
                    />
                    <span>
                      {batch.filename} into {accountName(batch.account_id)} —{" "}
                      {batch.created_count ?? 0} of {batch.row_count ?? 0} rows,{" "}
                      {batch.duplicate_count ?? 0} already here
                    </span>
                  </li>
                ))}
            </ul>
          )}
          <p className="text-sm text-muted-foreground">
            A CSV statement is imported under{" "}
            <a href={routeHash({ name: "settings", page: "import" })}>Import</a>; what arrives waits
            in Needs review until somebody looks at it.
          </p>
        </CardContent>
      </Card>

      {plaidReady ? (
        <Card aria-labelledby="plaid-title">
          <CardHeader>
            <h3 id="plaid-title" className="text-base font-semibold leading-none">
              Connect through Plaid
            </h3>
            <CardDescription>
              Link a real institution in Plaid&apos;s sandbox. The connection syncs on the same
              fifteen-minute schedule as everything else; no credential ever reaches this browser.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form
              className="grid gap-4"
              aria-label="Connect through Plaid"
              data-testid="plaid-connect"
              onSubmit={(event) => void connectPlaid(event)}
            >
              <div className="grid gap-4 sm:grid-cols-3">
                <Field label="Account" htmlFor="plaid-account">
                  <NativeSelect id="plaid-account" name="account_id" defaultValue="" required>
                    <option value="">Choose an account</option>
                    {accounts.map((account) => (
                      <option key={account.id} value={account.id}>
                        {account.name}
                      </option>
                    ))}
                  </NativeSelect>
                </Field>
              </div>
              <div className="flex gap-2">
                <Button type="submit" disabled={plaidBusy}>
                  <Landmark />
                  {plaidBusy ? "Linking…" : "Connect through Plaid"}
                </Button>
              </div>
            </form>
          </CardContent>
        </Card>
      ) : null}

      <Card aria-labelledby="connect-title">
        <CardHeader>
          <h3 id="connect-title" className="text-base font-semibold leading-none">
            Connect an account
          </h3>
          <CardDescription>
            The simulated institution: it hands the scheduled sync a few transactions for whatever
            account id it is asked about.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form
            className="grid gap-4"
            aria-label="Connect an account"
            onSubmit={(event) => void connect(event)}
          >
            <div className="grid gap-4 sm:grid-cols-3">
              <Field label="Account" htmlFor="connect-account">
                <NativeSelect id="connect-account" name="account_id" defaultValue="" required>
                  <option value="">Choose an account</option>
                  {accounts.map((account) => (
                    <option key={account.id} value={account.id}>
                      {account.name}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Field label="Institution" htmlFor="connect-institution">
                <Input
                  id="connect-institution"
                  name="institution"
                  required
                  maxLength={200}
                  defaultValue="Simulated Bank"
                />
              </Field>
              <Field label="Their account id" htmlFor="connect-external-id">
                <Input
                  id="connect-external-id"
                  name="external_id"
                  required
                  maxLength={64}
                  placeholder="acct-1"
                />
              </Field>
            </div>
            <div className="flex gap-2">
              <Button type="submit">
                <Plug />
                Connect
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
    </section>
  );
}

/** Where the household stands on being told about a failing sync, for the hint's attribute. */
function syncAlertState(setting: { readonly enabled?: boolean } | null): "unset" | "off" | "on" {
  if (setting === null) return "unset";
  return setting.enabled === false ? "off" : "on";
}
