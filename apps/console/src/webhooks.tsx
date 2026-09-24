import type {
  WebhookDelivery,
  WebhookDeliveryPage,
  WebhookDeliveryState,
  WebhookEndpoint,
  WebhookEndpointCreate,
  WebhookEndpointUpdate,
  WebhookEvent,
  WebhookSubscription,
} from "@mako-cloud/management-sdk";
import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Checkbox,
  cn,
  EmptyState,
  Eyebrow,
  Field,
  Input,
  Label,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { Webhook } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useId, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction, OneTimeSecretValue } from "./safety.js";

const EVENTS: readonly WebhookEvent[] = ["insert", "update", "delete"];
const DELIVERY_STATES: readonly WebhookDeliveryState[] = ["pending", "delivered", "failed"];
const DELIVERY_PAGE_SIZE = 50;
const COLLECTION_ID_PATTERN = "[a-z][a-z0-9_-]{0,62}";
const COLLECTION_ID = new RegExp(`^${COLLECTION_ID_PATTERN}$`, "u");

/** A code snippet inline in prose or a cell: an identifier, a URL, an error. */
const CODE = "rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]";
/** The quieter second line of a cell: an id, a description, a response status. */
const SUBLINE = "mt-0.5 block text-xs font-normal text-muted-foreground";
/** A term in the details list. */
const TERM = "m-0 text-xs font-medium tracking-wide text-muted-foreground uppercase";
/** Its definition. */
const DEFINITION = "m-0 mt-0.5 text-sm font-medium break-words tabular-nums";

/** One line of the subscription editor; `key` survives edits so React keeps the inputs. */
interface SubscriptionRow {
  readonly key: number;
  readonly collectionId: string;
  readonly events: readonly WebhookEvent[];
}

interface OneTimeSecret {
  readonly label: string;
  readonly value: string;
}

/** What the endpoint form yields once its fields are checked. */
interface EndpointInput {
  readonly url: string;
  readonly description: string;
  readonly subscriptions: WebhookSubscription[];
  readonly enabled: boolean;
}

// Webhook endpoints per environment and their deliveries, over the management
// API's webhook routes: the list with a registration form, or one endpoint's
// details, settings, secret, and delivery log. A signing secret appears once,
// in the answer that created or rotated it, and is never shown again.
export function WebhooksScreen({
  projectId,
  environmentId,
  webhookId,
  navigate,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly webhookId?: string | undefined;
  readonly navigate: (path: string) => void;
}) {
  const base = `/projects/${projectId}/environments/${environmentId}/webhooks`;
  return webhookId === undefined ? (
    <EndpointListScreen
      projectId={projectId}
      environmentId={environmentId}
      onOpen={(id) => navigate(`${base}/${id}`)}
    />
  ) : (
    <EndpointScreen
      projectId={projectId}
      environmentId={environmentId}
      webhookId={webhookId}
      onBack={() => navigate(base)}
    />
  );
}

function EndpointListScreen({
  projectId,
  environmentId,
  onOpen,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onOpen: (webhookId: string) => void;
}) {
  const client = useManagementClient();
  const [endpoints, setEndpoints] = useState<WebhookEndpoint[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [oneTime, setOneTime] = useState<OneTimeSecret | null>(null);
  const reload = useCallback(async () => {
    try {
      setEndpoints(await client.listWebhookEndpoints(projectId, environmentId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const create = async (input: EndpointInput): Promise<boolean> => {
    setCreating(true);
    setStatus(null);
    try {
      const request: WebhookEndpointCreate = {
        url: input.url,
        subscriptions: input.subscriptions,
        enabled: input.enabled,
        ...(input.description === "" ? {} : { description: input.description }),
      };
      const created = await client.createWebhookEndpoint(
        projectId,
        environmentId,
        request,
        idempotencyKey(),
      );
      setFailure(null);
      setStatus(`Webhook ${created.endpoint.id} registered.`);
      setOneTime({
        label: `signing secret for webhook ${created.endpoint.id}`,
        value: created.signingSecret,
      });
      await reload();
      return true;
    } catch (error) {
      setFailure(failureFrom(error));
      return false;
    } finally {
      setCreating(false);
    }
  };

  return (
    <section aria-labelledby="webhooks-title" className="grid gap-6">
      <div className="grid gap-1">
        <Eyebrow>Environment {environmentId}</Eyebrow>
        <h1 id="webhooks-title" className="text-2xl">
          Webhooks
        </h1>
        <p className="m-0 max-w-3xl text-sm text-muted-foreground">
          Signed HTTP deliveries to your own endpoints when documents in project{" "}
          <code className={CODE}>{projectId}</code> change: durable, retried with backoff, and
          paused rather than dropped when an endpoint keeps failing.
        </p>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : <StatusNotice>{status}</StatusNotice>}
      {oneTime === null ? null : (
        <OneTimeSecretValue
          label={oneTime.label}
          value={oneTime.value}
          onDismiss={() => setOneTime(null)}
        />
      )}
      <div className="grid items-start gap-6">
        <Card aria-labelledby="endpoint-list-title">
          <CardHeader>
            <CardTitle id="endpoint-list-title">Endpoints</CardTitle>
          </CardHeader>
          <CardContent>
            {endpoints === null && failure !== null ? (
              // A failed load used to leave "Loading endpoints…" up for good,
              // with nothing to do but reload the page.
              <LoadRetry what="Endpoints" onRetry={() => void reload()} />
            ) : endpoints === null ? (
              <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
                Loading endpoints…
              </p>
            ) : endpoints.length === 0 ? (
              <EmptyState
                icon={<Webhook aria-hidden="true" />}
                title="No webhook endpoints have been registered in this environment."
                className="border-0 py-8"
              />
            ) : (
              <Table>
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead scope="col">Endpoint</TableHead>
                    <TableHead scope="col">Subscriptions</TableHead>
                    <TableHead scope="col">State</TableHead>
                    <TableHead scope="col" className="text-right">
                      Failures
                    </TableHead>
                    <TableHead scope="col">Updated</TableHead>
                    <TableHead scope="col" className="text-right">
                      <span className="sr-only">Actions</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {endpoints.map((endpoint) => (
                    <TableRow key={endpoint.id} data-webhook-id={endpoint.id}>
                      <TableHead scope="row" className="max-w-xs align-top whitespace-normal">
                        <code className={cn(CODE, "break-all")}>{endpoint.url}</code>
                        {endpoint.description === "" ? null : (
                          <small className={SUBLINE}>{endpoint.description}</small>
                        )}
                      </TableHead>
                      <TableCell className="align-top">
                        <SubscriptionSummary subscriptions={endpoint.subscriptions} />
                      </TableCell>
                      <TableCell className="align-top whitespace-normal">
                        <EndpointState endpoint={endpoint} />
                      </TableCell>
                      <TableCell className="numeric text-right align-top tabular-nums">
                        {endpoint.consecutiveFailures.toLocaleString("en-US")}
                      </TableCell>
                      <TableCell className="align-top text-muted-foreground">
                        <Timestamp value={endpoint.updatedAt} />
                      </TableCell>
                      <TableCell className="text-right align-top">
                        <Button
                          variant="outline"
                          size="sm"
                          aria-label={`Open webhook ${endpoint.id}`}
                          onClick={() => onOpen(endpoint.id)}
                        >
                          Open
                        </Button>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>
        <Card aria-labelledby="register-endpoint-title" className="w-full max-w-3xl">
          <CardHeader>
            <CardTitle id="register-endpoint-title">Register endpoint</CardTitle>
            <CardDescription>
              The platform generates the signing secret and shows it once, right here, when the
              endpoint is registered. Deliveries begin with changes committed from then on.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <EndpointForm
              submitLabel="Register endpoint"
              busyLabel="Registering endpoint…"
              busy={creating}
              onSubmit={create}
              onFailure={setFailure}
            />
          </CardContent>
        </Card>
      </div>
    </section>
  );
}

function EndpointScreen({
  projectId,
  environmentId,
  webhookId,
  onBack,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly webhookId: string;
  readonly onBack: () => void;
}) {
  const client = useManagementClient();
  const filterId = useId();
  const [endpoint, setEndpoint] = useState<WebhookEndpoint | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [oneTime, setOneTime] = useState<OneTimeSecret | null>(null);
  const [saving, setSaving] = useState(false);
  const [acting, setActing] = useState<"enable" | "resume" | "rotate" | "delete" | null>(null);
  const [stateFilter, setStateFilter] = useState<WebhookDeliveryState | "">("");
  const [page, setPage] = useState<WebhookDeliveryPage | null>(null);
  const [loadingDeliveries, setLoadingDeliveries] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [redelivering, setRedelivering] = useState<string | null>(null);

  const reloadEndpoint = useCallback(async () => {
    try {
      setEndpoint(await client.getWebhookEndpoint(projectId, environmentId, webhookId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, environmentId, projectId, webhookId]);
  useEffect(() => {
    void reloadEndpoint();
  }, [reloadEndpoint]);

  useEffect(() => {
    let cancelled = false;
    setLoadingDeliveries(true);
    client
      .listWebhookDeliveries(projectId, environmentId, webhookId, deliveryQuery(stateFilter))
      .then((next) => {
        if (!cancelled) {
          setPage(next);
          setFailure(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setFailure(failureFrom(error));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoadingDeliveries(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client, environmentId, projectId, stateFilter, webhookId]);

  const save = async (input: EndpointInput): Promise<boolean> => {
    setSaving(true);
    setStatus(null);
    try {
      const request: WebhookEndpointUpdate = {
        url: input.url,
        description: input.description,
        subscriptions: input.subscriptions,
      };
      setEndpoint(
        await client.updateWebhookEndpoint(
          projectId,
          environmentId,
          webhookId,
          request,
          idempotencyKey(),
        ),
      );
      setFailure(null);
      setStatus("Webhook settings saved.");
      return true;
    } catch (error) {
      setFailure(failureFrom(error));
      return false;
    } finally {
      setSaving(false);
    }
  };

  const setEnabled = async (enabled: boolean) => {
    setActing("enable");
    setStatus(null);
    try {
      setEndpoint(
        await client.updateWebhookEndpoint(
          projectId,
          environmentId,
          webhookId,
          { enabled },
          idempotencyKey(),
        ),
      );
      setFailure(null);
      setStatus(
        enabled
          ? "Webhook enabled; new changes are delivered again."
          : "Webhook disabled; no new deliveries are queued until it is enabled.",
      );
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  const resume = async () => {
    setActing("resume");
    setStatus(null);
    try {
      setEndpoint(
        await client.resumeWebhookEndpoint(projectId, environmentId, webhookId, idempotencyKey()),
      );
      setFailure(null);
      setStatus("Webhook resumed; deliveries still pending are retried from where they stopped.");
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  const rotate = async () => {
    if (
      !confirmDestructiveAction({
        action: "Rotate the signing secret of webhook",
        target: webhookId,
        consequence:
          "Deliveries attempted after the rotation are signed with the new secret only. A receiver still verifying with the old secret rejects them until it is updated.",
      })
    ) {
      return;
    }
    setActing("rotate");
    setStatus(null);
    try {
      const rotated = await client.rotateWebhookSecret(
        projectId,
        environmentId,
        webhookId,
        idempotencyKey(),
      );
      setEndpoint(rotated.endpoint);
      setFailure(null);
      setStatus(
        `Signing secret rotated; version ${rotated.endpoint.secretVersion} signs from now.`,
      );
      setOneTime({
        label: `signing secret for webhook ${webhookId}`,
        value: rotated.signingSecret,
      });
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  const remove = async () => {
    if (
      !confirmDestructiveAction({
        action: "Delete webhook",
        target: webhookId,
        consequence:
          "The endpoint, its subscriptions, and its delivery log are removed. Pending deliveries are dropped. This cannot be undone.",
      })
    ) {
      return;
    }
    setActing("delete");
    setStatus(null);
    try {
      await client.deleteWebhookEndpoint(projectId, environmentId, webhookId, idempotencyKey());
      onBack();
    } catch (error) {
      setFailure(failureFrom(error));
      setActing(null);
    }
  };

  const loadMore = async () => {
    if (page === null || page.nextCursor === null) {
      return;
    }
    setLoadingMore(true);
    try {
      const next = await client.listWebhookDeliveries(
        projectId,
        environmentId,
        webhookId,
        deliveryQuery(stateFilter, page.nextCursor),
      );
      setPage({ ...next, items: [...page.items, ...next.items] });
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setLoadingMore(false);
    }
  };

  const redeliver = async (delivery: WebhookDelivery) => {
    setRedelivering(delivery.id);
    setStatus(null);
    try {
      const queued = await client.redeliverWebhookDelivery(
        projectId,
        environmentId,
        webhookId,
        delivery.id,
        idempotencyKey(),
      );
      setPage((current) =>
        current === null ? current : { ...current, items: [queued, ...current.items] },
      );
      setFailure(null);
      setStatus(`Delivery ${queued.id} queued as a redelivery of ${delivery.id}.`);
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setRedelivering(null);
    }
  };

  const busy = acting !== null;
  return (
    <section aria-labelledby="webhook-title" className="grid gap-6">
      <div className="grid gap-2">
        <Button
          variant="ghost"
          size="sm"
          className="-ml-2 w-fit text-muted-foreground hover:text-foreground"
          onClick={onBack}
        >
          ← Webhooks
        </Button>
        <div className="grid gap-1">
          <Eyebrow>Webhook endpoint</Eyebrow>
          <h1 id="webhook-title" className="font-mono text-2xl">
            {webhookId}
          </h1>
          {endpoint === null ? null : (
            <p className="m-0 text-sm text-muted-foreground">
              Deliveries are posted to <code className={cn(CODE, "break-all")}>{endpoint.url}</code>
            </p>
          )}
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : <StatusNotice>{status}</StatusNotice>}
      {oneTime === null ? null : (
        <OneTimeSecretValue
          label={oneTime.label}
          value={oneTime.value}
          onDismiss={() => setOneTime(null)}
        />
      )}
      {endpoint === null && failure !== null ? (
        <LoadRetry what="This webhook" onRetry={() => void reloadEndpoint()} />
      ) : endpoint === null ? (
        <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
          Loading webhook…
        </p>
      ) : (
        <div className="grid items-start gap-6 lg:grid-cols-2">
          <Card aria-labelledby="webhook-details-title">
            <CardHeader>
              <CardTitle id="webhook-details-title">Details</CardTitle>
            </CardHeader>
            <CardContent className="grid gap-5">
              <dl className="m-0 grid gap-x-6 gap-y-4 sm:grid-cols-2">
                <div>
                  <dt className={TERM}>State</dt>
                  <dd className={DEFINITION} data-field="state">
                    <EndpointState endpoint={endpoint} />
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Enabled</dt>
                  <dd className={DEFINITION} data-field="enabled">
                    {endpoint.enabled ? "yes" : "no"}
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Consecutive failures</dt>
                  <dd className={DEFINITION} data-field="consecutiveFailures">
                    {endpoint.consecutiveFailures.toLocaleString("en-US")}
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Secret version</dt>
                  <dd className={DEFINITION} data-field="secretVersion">
                    {endpoint.secretVersion}
                  </dd>
                </div>
                {endpoint.pausedAt === null ? null : (
                  <div>
                    <dt className={TERM}>Paused at</dt>
                    <dd className={DEFINITION} data-field="pausedAt">
                      <Timestamp value={endpoint.pausedAt} />
                    </dd>
                  </div>
                )}
                <div>
                  <dt className={TERM}>Created</dt>
                  <dd className={DEFINITION}>
                    <Timestamp value={endpoint.createdAt} />
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Updated</dt>
                  <dd className={DEFINITION}>
                    <Timestamp value={endpoint.updatedAt} />
                  </dd>
                </div>
              </dl>
              <p className="m-0 text-sm text-muted-foreground">
                Deliveries carry the signature header named by the secret version. The secret itself
                cannot be read back; rotating it shows the new one once.
              </p>
              <div className="flex flex-wrap gap-2">
                {endpoint.state === "paused" ? (
                  <Button disabled={busy} onClick={() => void resume()}>
                    {acting === "resume" ? "Resuming…" : "Resume"}
                  </Button>
                ) : null}
                <Button
                  variant="outline"
                  disabled={busy}
                  onClick={() => void setEnabled(!endpoint.enabled)}
                >
                  {acting === "enable"
                    ? endpoint.enabled
                      ? "Disabling…"
                      : "Enabling…"
                    : endpoint.enabled
                      ? "Disable"
                      : "Enable"}
                </Button>
                <Button variant="outline" disabled={busy} onClick={() => void rotate()}>
                  {acting === "rotate" ? "Rotating secret…" : "Rotate secret"}
                </Button>
                <Button
                  variant="outline"
                  className="border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive"
                  disabled={busy}
                  onClick={() => void remove()}
                >
                  {acting === "delete" ? "Deleting…" : "Delete"}
                </Button>
              </div>
            </CardContent>
          </Card>
          <Card aria-labelledby="webhook-settings-title">
            <CardHeader>
              <CardTitle id="webhook-settings-title">Settings</CardTitle>
              <CardDescription>
                Changes apply to deliveries queued from now on; the signing secret is kept.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <EndpointForm
                key={`settings-${endpoint.updatedAt}`}
                endpoint={endpoint}
                submitLabel="Save"
                busyLabel="Saving…"
                busy={saving}
                onSubmit={save}
                onFailure={setFailure}
              />
            </CardContent>
          </Card>
          <Card aria-labelledby="webhook-deliveries-title" className="lg:col-span-2">
            <CardHeader>
              <CardTitle id="webhook-deliveries-title">Deliveries</CardTitle>
              <CardDescription>
                Newest first. The log is retained for a bounded period.
              </CardDescription>
            </CardHeader>
            <CardContent className="grid gap-4">
              <Field label="State" htmlFor={filterId} className="w-56">
                <NativeSelect
                  id={filterId}
                  name="state"
                  size="sm"
                  value={stateFilter}
                  onChange={(event) =>
                    setStateFilter(event.currentTarget.value as WebhookDeliveryState | "")
                  }
                >
                  <option value="">any state</option>
                  {DELIVERY_STATES.map((state) => (
                    <option key={state} value={state}>
                      {state}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              {loadingDeliveries && page === null ? (
                <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
                  Loading deliveries…
                </p>
              ) : page === null || page.items.length === 0 ? (
                <p className="m-0 text-sm text-muted-foreground">
                  {stateFilter === ""
                    ? "No deliveries have been logged for this endpoint."
                    : `No ${stateFilter} deliveries are logged.`}
                </p>
              ) : (
                <DeliveryTable
                  deliveries={page.items}
                  redelivering={redelivering}
                  canRedeliver={endpoint.state === "active"}
                  onRedeliver={(delivery) => void redeliver(delivery)}
                />
              )}
              <div className="flex flex-wrap items-center justify-between gap-3">
                <small className="text-xs text-muted-foreground">
                  {page === null || page.nextCursor === null
                    ? "Every retained delivery is listed."
                    : "Older deliveries are retained."}
                </small>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={
                    page === null || page.nextCursor === null || loadingDeliveries || loadingMore
                  }
                  onClick={() => void loadMore()}
                >
                  {loadingMore ? "Loading…" : "Load more"}
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>
      )}
    </section>
  );
}

function DeliveryTable({
  deliveries,
  redelivering,
  canRedeliver,
  onRedeliver,
}: {
  readonly deliveries: readonly WebhookDelivery[];
  readonly redelivering: string | null;
  readonly canRedeliver: boolean;
  readonly onRedeliver: (delivery: WebhookDelivery) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col">Time</TableHead>
          <TableHead scope="col">Event</TableHead>
          <TableHead scope="col">Collection</TableHead>
          <TableHead scope="col">Document</TableHead>
          <TableHead scope="col" className="text-right">
            Attempts
          </TableHead>
          <TableHead scope="col">Status</TableHead>
          <TableHead scope="col">Error</TableHead>
          <TableHead scope="col" className="text-right">
            <span className="sr-only">Actions</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {deliveries.map((delivery) => (
          <TableRow key={delivery.id} data-delivery-id={delivery.id} data-state={delivery.state}>
            <TableHead scope="row" className="align-top">
              <Timestamp value={delivery.createdAt} />
              <small className={cn(SUBLINE, "font-mono")}>{delivery.id}</small>
            </TableHead>
            <TableCell className="align-top">
              {delivery.event}
              {delivery.redeliveryOf === null ? null : (
                <small className={SUBLINE}>redelivery of {delivery.redeliveryOf}</small>
              )}
            </TableCell>
            <TableCell className="align-top">
              <code className={CODE}>{delivery.collectionId}</code>
            </TableCell>
            <TableCell className="align-top whitespace-normal">
              <code className={cn(CODE, "break-all")}>{delivery.documentId}</code>
              <small className={cn(SUBLINE, "font-mono")}>revision {delivery.revision}</small>
            </TableCell>
            <TableCell className="numeric text-right align-top tabular-nums">
              {delivery.attempts.toLocaleString("en-US")}
            </TableCell>
            <TableCell className="align-top">
              <Badge
                className="delivery-state capitalize"
                variant={deliveryVariant(delivery.state)}
              >
                {delivery.state}
              </Badge>
              {delivery.lastResponseStatus === null ? null : (
                <small className={cn(SUBLINE, "tabular-nums")}>
                  HTTP {delivery.lastResponseStatus}
                </small>
              )}
              {delivery.state === "pending" && delivery.nextAttemptAt !== null ? (
                <small className={SUBLINE}>
                  next attempt <Timestamp value={delivery.nextAttemptAt} />
                </small>
              ) : null}
            </TableCell>
            <TableCell className="align-top whitespace-normal">
              {delivery.lastError === null ? (
                <em className="text-muted-foreground">none</em>
              ) : (
                <code className={cn(CODE, "break-all")}>{delivery.lastError}</code>
              )}
            </TableCell>
            <TableCell className="text-right align-top">
              {delivery.state === "failed" ? (
                <Button
                  variant="outline"
                  size="sm"
                  aria-label={`Redeliver ${delivery.id}`}
                  disabled={redelivering !== null || !canRedeliver}
                  title={
                    canRedeliver
                      ? undefined
                      : "Redelivery is refused while the endpoint is paused or disabled."
                  }
                  onClick={() => onRedeliver(delivery)}
                >
                  {redelivering === delivery.id ? "Queuing…" : "Redeliver"}
                </Button>
              ) : null}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/**
 * The endpoint's URL, description, and subscriptions, shared by the
 * registration form (which also asks whether to deliver at once) and the
 * settings form. The form checks its own fields and hands the parent a
 * well-formed input; the parent decides what to send and what to say.
 */
function EndpointForm({
  endpoint,
  submitLabel,
  busyLabel,
  busy,
  onSubmit,
  onFailure,
}: {
  readonly endpoint?: WebhookEndpoint;
  readonly submitLabel: string;
  readonly busyLabel: string;
  readonly busy: boolean;
  readonly onSubmit: (input: EndpointInput) => Promise<boolean>;
  readonly onFailure: (failure: ConsoleApiFailure) => void;
}) {
  const id = useId();
  const [rows, setRows] = useState<SubscriptionRow[]>(() =>
    endpoint === undefined ? [emptyRow(0)] : endpoint.subscriptions.map(rowFrom),
  );
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    let input: EndpointInput;
    try {
      input = {
        url: parseUrl(String(data.get("url") ?? "")),
        description: String(data.get("description") ?? "").trim(),
        subscriptions: subscriptionsFrom(rows),
        enabled: endpoint === undefined ? data.get("enabled") === "on" : endpoint.enabled,
      };
    } catch (error) {
      onFailure(failureFrom(error));
      return;
    }
    if ((await onSubmit(input)) && endpoint === undefined) {
      form.reset();
      setRows([emptyRow(0)]);
    }
  };
  const update = (key: number, change: Partial<Omit<SubscriptionRow, "key">>) =>
    setRows((current) => current.map((row) => (row.key === key ? { ...row, ...change } : row)));
  const toggle = (row: SubscriptionRow, event: WebhookEvent, checked: boolean) =>
    update(row.key, {
      events: checked
        ? EVENTS.filter((candidate) => candidate === event || row.events.includes(candidate))
        : row.events.filter((candidate) => candidate !== event),
    });

  return (
    <form className="grid gap-4" onSubmit={(event) => void submit(event)}>
      <Field label="Endpoint URL" htmlFor={`${id}-url`}>
        <Input
          id={`${id}-url`}
          name="url"
          type="url"
          required
          placeholder="https://example.test/hooks/mako"
          spellCheck={false}
          defaultValue={endpoint?.url ?? ""}
          className="font-mono"
        />
        <p className="m-0 text-sm text-muted-foreground">
          An absolute <code className={CODE}>https</code> URL; <code className={CODE}>http</code> is
          admitted only to loopback outside production.
        </p>
      </Field>
      <Field label="Description" htmlFor={`${id}-description`}>
        <Input
          id={`${id}-description`}
          name="description"
          maxLength={256}
          placeholder="orders to the warehouse"
          defaultValue={endpoint?.description ?? ""}
        />
      </Field>
      <fieldset className="m-0 grid min-w-0 gap-3 rounded-lg border p-3">
        <legend className="px-1 text-sm font-medium">Subscriptions</legend>
        {rows.map((row, index) => (
          <div
            className="grid items-center gap-x-3 gap-y-2 sm:grid-cols-[minmax(8rem,1fr)_repeat(3,auto)_auto]"
            key={row.key}
          >
            <Input
              type="text"
              aria-label={`Subscription ${index + 1} collection`}
              placeholder="collection id"
              pattern={COLLECTION_ID_PATTERN}
              spellCheck={false}
              className="font-mono"
              value={row.collectionId}
              onChange={(event) => update(row.key, { collectionId: event.currentTarget.value })}
            />
            {EVENTS.map((event) => (
              <div className="flex items-center gap-2 whitespace-nowrap" key={event}>
                <Checkbox
                  id={`${id}-${row.key}-${event}`}
                  aria-label={`Subscription ${index + 1} ${event}`}
                  checked={row.events.includes(event)}
                  onCheckedChange={(checked) => toggle(row, event, checked === true)}
                />
                <Label htmlFor={`${id}-${row.key}-${event}`} className="font-normal">
                  {event}
                </Label>
              </div>
            ))}
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-foreground"
              aria-label={`Remove subscription ${index + 1}`}
              disabled={rows.length === 1}
              onClick={() => setRows((current) => current.filter((item) => item.key !== row.key))}
            >
              Remove
            </Button>
          </div>
        ))}
        <div>
          <Button
            variant="outline"
            size="sm"
            onClick={() =>
              setRows((current) => [
                ...current,
                emptyRow(1 + Math.max(...current.map((row) => row.key))),
              ])
            }
          >
            Add collection
          </Button>
        </div>
      </fieldset>
      <p className="-mt-2 text-sm text-muted-foreground">
        Each collection is delivered for the events ticked; a collection appears once.
      </p>
      {endpoint === undefined ? (
        <div className="flex items-center gap-2">
          <Checkbox id={`${id}-enabled`} name="enabled" defaultChecked />
          <Label htmlFor={`${id}-enabled`} className="font-normal">
            Enabled — deliver as soon as the endpoint is registered
          </Label>
        </div>
      ) : null}
      <div>
        <Button type="submit" disabled={busy}>
          {busy ? busyLabel : submitLabel}
        </Button>
      </div>
    </form>
  );
}

function SubscriptionSummary({
  subscriptions,
}: {
  readonly subscriptions: readonly WebhookSubscription[];
}) {
  if (subscriptions.length === 0) {
    return <em className="text-muted-foreground">none</em>;
  }
  return (
    <ul className="m-0 grid list-none gap-1 p-0">
      {subscriptions.map((subscription) => (
        <li key={subscription.collectionId} className="whitespace-nowrap">
          <code className={CODE}>{subscription.collectionId}</code>{" "}
          <span className="text-muted-foreground">{subscription.events.join(", ")}</span>
        </li>
      ))}
    </ul>
  );
}

/** The state as a badge; a pause carries the platform's reason beside it. */
function EndpointState({ endpoint }: { readonly endpoint: WebhookEndpoint }) {
  return (
    <>
      <Badge className="webhook-state capitalize" variant={endpointVariant(endpoint.state)}>
        {endpoint.state}
      </Badge>
      {endpoint.state === "paused" && endpoint.pausedReason !== null ? (
        <small className="paused-reason mt-1 block text-xs font-normal text-destructive">
          {endpoint.pausedReason}
        </small>
      ) : null}
    </>
  );
}

function endpointVariant(
  state: WebhookEndpoint["state"],
): "positive" | "destructive" | "secondary" {
  switch (state) {
    case "active":
      return "positive";
    case "paused":
      return "destructive";
    case "disabled":
      return "secondary";
  }
}

function deliveryVariant(state: WebhookDeliveryState): "positive" | "destructive" | "warning" {
  switch (state) {
    case "delivered":
      return "positive";
    case "failed":
      return "destructive";
    case "pending":
      return "warning";
  }
}

/** What the last action did, in the page's own words. */
function StatusNotice({ children }: { readonly children: ReactNode }) {
  return (
    <Alert variant="positive" role="status">
      <AlertDescription className="block text-foreground">{children}</AlertDescription>
    </Alert>
  );
}

function Timestamp({ value }: { readonly value: string }) {
  return (
    <time dateTime={value} className="whitespace-nowrap tabular-nums">
      {new Date(value).toLocaleString()}
    </time>
  );
}

function deliveryQuery(
  state: WebhookDeliveryState | "",
  cursor?: string,
): { readonly state?: WebhookDeliveryState; readonly cursor?: string; readonly limit?: number } {
  return {
    limit: DELIVERY_PAGE_SIZE,
    ...(state === "" ? {} : { state }),
    ...(cursor === undefined ? {} : { cursor }),
  };
}

function emptyRow(key: number): SubscriptionRow {
  return { key, collectionId: "", events: [...EVENTS] };
}

function rowFrom(subscription: WebhookSubscription, index: number): SubscriptionRow {
  return { key: index, collectionId: subscription.collectionId, events: subscription.events };
}

function parseUrl(raw: string): string {
  const trimmed = raw.trim();
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    throw new FormInputError("Endpoint URL must be an absolute URL.");
  }
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
    throw new FormInputError("Endpoint URL must use https.");
  }
  return trimmed;
}

/** The editor's rows as subscriptions, events in canonical order, each collection once. */
export function subscriptionsFrom(
  rows: readonly { readonly collectionId: string; readonly events: readonly WebhookEvent[] }[],
): WebhookSubscription[] {
  const seen = new Set<string>();
  return rows.map((row, index) => {
    const position = `Subscription ${index + 1}`;
    const collectionId = row.collectionId.trim();
    if (!COLLECTION_ID.test(collectionId)) {
      throw new FormInputError(
        `${position} needs a collection id: lowercase letters, digits, - and _, starting with a letter.`,
      );
    }
    if (seen.has(collectionId)) {
      throw new FormInputError(
        `Collection ${collectionId} is listed twice; keep its events on one row.`,
      );
    }
    seen.add(collectionId);
    const events = EVENTS.filter((event) => row.events.includes(event));
    if (events.length === 0) {
      throw new FormInputError(`${position} (${collectionId}) needs at least one event.`);
    }
    return { collectionId, events };
  });
}

class FormInputError extends Error {}

/** What a view shows when its first load failed: the failure is above it. */
function LoadRetry({ what, onRetry }: { readonly what: string; readonly onRetry: () => void }) {
  return (
    <div className="grid justify-items-start gap-3">
      <p className="m-0 text-sm text-muted-foreground">{what} could not be loaded.</p>
      <Button type="button" size="sm" variant="outline" onClick={onRetry}>
        Try again
      </Button>
    </div>
  );
}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
