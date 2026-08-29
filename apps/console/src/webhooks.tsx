import { type FormEvent, useCallback, useEffect, useState } from "react";

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

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction, OneTimeSecretValue } from "./safety.js";

const EVENTS: readonly WebhookEvent[] = ["insert", "update", "delete"];
const DELIVERY_STATES: readonly WebhookDeliveryState[] = ["pending", "delivered", "failed"];
const DELIVERY_PAGE_SIZE = 50;
const COLLECTION_ID_PATTERN = "[a-z][a-z0-9_-]{0,62}";
const COLLECTION_ID = new RegExp(`^${COLLECTION_ID_PATTERN}$`, "u");

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
    <section aria-labelledby="webhooks-title" className="webhooks-screen">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="webhooks-title">Webhooks</h1>
          <p>
            Signed HTTP deliveries to your own endpoints when documents in project{" "}
            <code>{projectId}</code> change: durable, retried with backoff, and paused rather than
            dropped when an endpoint keeps failing.
          </p>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
      {oneTime === null ? null : (
        <OneTimeSecretValue
          label={oneTime.label}
          value={oneTime.value}
          onDismiss={() => setOneTime(null)}
        />
      )}
      <div className="split-grid">
        <section className="panel" aria-labelledby="endpoint-list-title">
          <h2 id="endpoint-list-title">Endpoints</h2>
          {endpoints === null ? (
            <p aria-live="polite">Loading endpoints…</p>
          ) : endpoints.length === 0 ? (
            <p>No webhook endpoints have been registered in this environment.</p>
          ) : (
            <div className="table-scroll">
              <table className="webhook-table">
                <thead>
                  <tr>
                    <th scope="col">Endpoint</th>
                    <th scope="col">Subscriptions</th>
                    <th scope="col">State</th>
                    <th scope="col" className="numeric">
                      Failures
                    </th>
                    <th scope="col">Updated</th>
                    <th scope="col">
                      <span className="visually-hidden">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {endpoints.map((endpoint) => (
                    <tr key={endpoint.id} data-webhook-id={endpoint.id}>
                      <th scope="row">
                        <code className="webhook-url">{endpoint.url}</code>
                        {endpoint.description === "" ? null : (
                          <small className="webhook-description">{endpoint.description}</small>
                        )}
                      </th>
                      <td>
                        <SubscriptionSummary subscriptions={endpoint.subscriptions} />
                      </td>
                      <td>
                        <EndpointState endpoint={endpoint} />
                      </td>
                      <td className="numeric">
                        {endpoint.consecutiveFailures.toLocaleString("en-US")}
                      </td>
                      <td>
                        <Timestamp value={endpoint.updatedAt} />
                      </td>
                      <td>
                        <button
                          type="button"
                          className="secondary"
                          aria-label={`Open webhook ${endpoint.id}`}
                          onClick={() => onOpen(endpoint.id)}
                        >
                          Open
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
        <section className="panel" aria-labelledby="register-endpoint-title">
          <h2 id="register-endpoint-title">Register endpoint</h2>
          <p>
            The platform generates the signing secret and shows it once, right here, when the
            endpoint is registered. Deliveries begin with changes committed from then on.
          </p>
          <EndpointForm
            submitLabel="Register endpoint"
            busyLabel="Registering endpoint…"
            busy={creating}
            onSubmit={create}
            onFailure={setFailure}
          />
        </section>
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
    <section aria-labelledby="webhook-title" className="webhooks-screen">
      <button type="button" className="back-link" onClick={onBack}>
        ← Webhooks
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Webhook endpoint</p>
          <h1 id="webhook-title">{webhookId}</h1>
          {endpoint === null ? null : (
            <p>
              Deliveries are posted to <code className="webhook-url">{endpoint.url}</code>
            </p>
          )}
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
      {oneTime === null ? null : (
        <OneTimeSecretValue
          label={oneTime.label}
          value={oneTime.value}
          onDismiss={() => setOneTime(null)}
        />
      )}
      {endpoint === null ? (
        <p aria-live="polite">Loading webhook…</p>
      ) : (
        <div className="split-grid">
          <section className="panel" aria-labelledby="webhook-details-title">
            <h2 id="webhook-details-title">Details</h2>
            <dl className="definition-grid webhook-details">
              <div>
                <dt>State</dt>
                <dd data-field="state">
                  <EndpointState endpoint={endpoint} />
                </dd>
              </div>
              <div>
                <dt>Enabled</dt>
                <dd data-field="enabled">{endpoint.enabled ? "yes" : "no"}</dd>
              </div>
              <div>
                <dt>Consecutive failures</dt>
                <dd data-field="consecutiveFailures">
                  {endpoint.consecutiveFailures.toLocaleString("en-US")}
                </dd>
              </div>
              <div>
                <dt>Secret version</dt>
                <dd data-field="secretVersion">{endpoint.secretVersion}</dd>
              </div>
              {endpoint.pausedAt === null ? null : (
                <div>
                  <dt>Paused at</dt>
                  <dd data-field="pausedAt">
                    <Timestamp value={endpoint.pausedAt} />
                  </dd>
                </div>
              )}
              <div>
                <dt>Created</dt>
                <dd>
                  <Timestamp value={endpoint.createdAt} />
                </dd>
              </div>
              <div>
                <dt>Updated</dt>
                <dd>
                  <Timestamp value={endpoint.updatedAt} />
                </dd>
              </div>
            </dl>
            <p>
              Deliveries carry the signature header named by the secret version. The secret itself
              cannot be read back; rotating it shows the new one once.
            </p>
            <div className="button-row">
              {endpoint.state === "paused" ? (
                <button type="button" disabled={busy} onClick={() => void resume()}>
                  {acting === "resume" ? "Resuming…" : "Resume"}
                </button>
              ) : null}
              <button
                type="button"
                className="secondary"
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
              </button>
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => void rotate()}
              >
                {acting === "rotate" ? "Rotating secret…" : "Rotate secret"}
              </button>
              <button
                type="button"
                className="danger"
                disabled={busy}
                onClick={() => void remove()}
              >
                {acting === "delete" ? "Deleting…" : "Delete"}
              </button>
            </div>
          </section>
          <section className="panel" aria-labelledby="webhook-settings-title">
            <h2 id="webhook-settings-title">Settings</h2>
            <p>Changes apply to deliveries queued from now on; the signing secret is kept.</p>
            <EndpointForm
              key={`settings-${endpoint.updatedAt}`}
              endpoint={endpoint}
              submitLabel="Save"
              busyLabel="Saving…"
              busy={saving}
              onSubmit={save}
              onFailure={setFailure}
            />
          </section>
          <section className="panel full-span" aria-labelledby="webhook-deliveries-title">
            <div className="button-row spread">
              <h2 id="webhook-deliveries-title">Deliveries</h2>
              <small>Newest first. The log is retained for a bounded period.</small>
            </div>
            <div className="delivery-filter">
              <label>
                State
                <select
                  name="state"
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
                </select>
              </label>
            </div>
            {loadingDeliveries && page === null ? (
              <p aria-live="polite">Loading deliveries…</p>
            ) : page === null || page.items.length === 0 ? (
              <p className="delivery-empty">
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
            <div className="button-row spread">
              <small>
                {page === null || page.nextCursor === null
                  ? "Every retained delivery is listed."
                  : "Older deliveries are retained."}
              </small>
              <button
                type="button"
                className="secondary"
                disabled={
                  page === null || page.nextCursor === null || loadingDeliveries || loadingMore
                }
                onClick={() => void loadMore()}
              >
                {loadingMore ? "Loading…" : "Load more"}
              </button>
            </div>
          </section>
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
    <div className="table-scroll">
      <table className="delivery-table">
        <thead>
          <tr>
            <th scope="col">Time</th>
            <th scope="col">Event</th>
            <th scope="col">Collection</th>
            <th scope="col">Document</th>
            <th scope="col" className="numeric">
              Attempts
            </th>
            <th scope="col">Status</th>
            <th scope="col">Error</th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {deliveries.map((delivery) => (
            <tr key={delivery.id} data-delivery-id={delivery.id} data-state={delivery.state}>
              <th scope="row">
                <Timestamp value={delivery.createdAt} />
                <small className="delivery-id">{delivery.id}</small>
              </th>
              <td>
                {delivery.event}
                {delivery.redeliveryOf === null ? null : (
                  <small className="delivery-id">redelivery of {delivery.redeliveryOf}</small>
                )}
              </td>
              <td>
                <code>{delivery.collectionId}</code>
              </td>
              <td>
                <code className="delivery-document">{delivery.documentId}</code>
                <small className="delivery-id">revision {delivery.revision}</small>
              </td>
              <td className="numeric">{delivery.attempts.toLocaleString("en-US")}</td>
              <td>
                <span className={`delivery-state ${delivery.state}`}>{delivery.state}</span>
                {delivery.lastResponseStatus === null ? null : (
                  <small className="delivery-id">HTTP {delivery.lastResponseStatus}</small>
                )}
                {delivery.state === "pending" && delivery.nextAttemptAt !== null ? (
                  <small className="delivery-id">
                    next attempt <Timestamp value={delivery.nextAttemptAt} />
                  </small>
                ) : null}
              </td>
              <td>
                {delivery.lastError === null ? (
                  <em>none</em>
                ) : (
                  <code className="delivery-error">{delivery.lastError}</code>
                )}
              </td>
              <td>
                {delivery.state === "failed" ? (
                  <button
                    type="button"
                    className="secondary"
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
                  </button>
                ) : null}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
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
    <form onSubmit={(event) => void submit(event)}>
      <label>
        Endpoint URL
        <input
          name="url"
          type="url"
          required
          placeholder="https://example.test/hooks/mako"
          spellCheck={false}
          defaultValue={endpoint?.url ?? ""}
        />
      </label>
      <small className="field-hint">
        An absolute <code>https</code> URL; <code>http</code> is admitted only to loopback outside
        production.
      </small>
      <label>
        Description
        <input
          name="description"
          maxLength={256}
          placeholder="orders to the warehouse"
          defaultValue={endpoint?.description ?? ""}
        />
      </label>
      <fieldset className="subscription-editor">
        <legend>Subscriptions</legend>
        {rows.map((row, index) => (
          <div className="subscription-row" key={row.key}>
            <input
              type="text"
              aria-label={`Subscription ${index + 1} collection`}
              placeholder="collection id"
              pattern={COLLECTION_ID_PATTERN}
              spellCheck={false}
              value={row.collectionId}
              onChange={(event) => update(row.key, { collectionId: event.currentTarget.value })}
            />
            {EVENTS.map((event) => (
              <label className="checkbox-label" key={event}>
                <input
                  type="checkbox"
                  aria-label={`Subscription ${index + 1} ${event}`}
                  checked={row.events.includes(event)}
                  onChange={(change) => toggle(row, event, change.currentTarget.checked)}
                />
                {event}
              </label>
            ))}
            <button
              type="button"
              className="secondary"
              aria-label={`Remove subscription ${index + 1}`}
              disabled={rows.length === 1}
              onClick={() => setRows((current) => current.filter((item) => item.key !== row.key))}
            >
              Remove
            </button>
          </div>
        ))}
        <button
          type="button"
          className="secondary"
          onClick={() =>
            setRows((current) => [
              ...current,
              emptyRow(1 + Math.max(...current.map((row) => row.key))),
            ])
          }
        >
          Add collection
        </button>
      </fieldset>
      <small className="field-hint">
        Each collection is delivered for the events ticked; a collection appears once.
      </small>
      {endpoint === undefined ? (
        <label className="checkbox-label">
          <input name="enabled" type="checkbox" defaultChecked />
          Enabled — deliver as soon as the endpoint is registered
        </label>
      ) : null}
      <button type="submit" disabled={busy}>
        {busy ? busyLabel : submitLabel}
      </button>
    </form>
  );
}

function SubscriptionSummary({
  subscriptions,
}: {
  readonly subscriptions: readonly WebhookSubscription[];
}) {
  if (subscriptions.length === 0) {
    return <em>none</em>;
  }
  return (
    <ul className="subscription-summary">
      {subscriptions.map((subscription) => (
        <li key={subscription.collectionId}>
          <code>{subscription.collectionId}</code> {subscription.events.join(", ")}
        </li>
      ))}
    </ul>
  );
}

/** The state as a badge; a pause carries the platform's reason beside it. */
function EndpointState({ endpoint }: { readonly endpoint: WebhookEndpoint }) {
  return (
    <>
      <span className={`webhook-state ${endpoint.state}`}>{endpoint.state}</span>
      {endpoint.state === "paused" && endpoint.pausedReason !== null ? (
        <small className="paused-reason">{endpoint.pausedReason}</small>
      ) : null}
    </>
  );
}

function Timestamp({ value }: { readonly value: string }) {
  return <time dateTime={value}>{new Date(value).toLocaleString()}</time>;
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

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
