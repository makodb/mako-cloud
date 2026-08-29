import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import type {
  ActivePolicy,
  AuthSettings,
  Collection,
  CollectionIndex,
  Function as EdgeFunction,
  PolicySet,
  StorageBucket,
} from "@mako-cloud/management-sdk";
import { createMakoRxdbConnectTemplateV1 } from "@mako-cloud/rxdb";

import { type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { RequestId } from "./error-boundary.js";
import { useManagementClient } from "./management.js";

// Generated per-environment API documentation. Everything on this screen is
// derived in the browser from what the console already reads -- collections
// with their schemas, indexes, and active policies, functions, buckets, auth
// settings, and the environment's public connection metadata -- and stamped
// with the time it was observed. Nothing is stored and nothing is rendered by
// the server. The only key material that may appear is a public project key;
// service credentials are refused before they can reach a snippet.

const PUBLIC_KEY_PREFIX = "mako_pk.";
const SERVICE_KEY_PREFIX = "mako_sk.";
const PUBLIC_KEY_PLACEHOLDER = "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY";
const API_URL_PLACEHOLDER = "https://API_URL";
const DOCUMENT_OPERATIONS = ["create", "read", "update", "delete"] as const;
const SAMPLE_TIME = "2026-01-01T00:00:00.000Z";
const SAMPLE_REDIRECT_URL = "https://app.example.com/auth/callback";
const SAMPLE_USER_ID = "usr_0123456789abcdef";
const SAMPLE_REVISION = "1-9f2c4b7e";
const QUICKSTART_CLIENTS = [
  { id: "curl", label: "curl" },
  { id: "javascript", label: "JavaScript (fetch)" },
  { id: "rxdb", label: "RxDB replication" },
] as const;

type DocumentOperation = (typeof DOCUMENT_OPERATIONS)[number];
type PolicyRule = PolicySet["rules"][number];
type QuickstartClient = (typeof QUICKSTART_CLIENTS)[number]["id"];

type Part<T> =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly value: T }
  | { readonly status: "unavailable"; readonly failure: ConsoleApiFailure };

interface CollectionReference {
  readonly collection: Collection;
  readonly indexes: Part<CollectionIndex[]>;
  readonly policy: Part<ActivePolicy>;
}

interface ConnectReference {
  readonly apiUrl: string;
  readonly publicKeyId: string;
  /** The key's value, only when the console already holds a public one. */
  readonly publicKey: string | null;
  readonly rxdbClientRange: string;
}

interface GeneratedDocs {
  readonly generatedAt: string;
  readonly collections: Part<CollectionReference[]>;
  readonly functions: Part<EdgeFunction[]>;
  readonly buckets: Part<StorageBucket[]>;
  readonly auth: Part<AuthSettings>;
  readonly connect: Part<ConnectReference>;
}

/** What every example is built against for this environment. */
interface DocsContext {
  readonly projectId: string;
  readonly environmentId: string;
  readonly apiUrl: string;
  readonly publicKeyId: string | null;
  readonly keyMaterial: string;
  readonly providerName: string;
  readonly redirectUrl: string;
  readonly magicLinksEnabled: boolean | null;
}

interface ExampleRequest {
  readonly id: string;
  readonly title: string;
  readonly method: "GET" | "POST" | "PUT" | "DELETE";
  readonly url: string;
  readonly headers: readonly (readonly [string, string])[];
  readonly body?: unknown;
  /** Verbatim curl data argument for non-JSON bodies. */
  readonly rawData?: string;
  readonly response: unknown;
  readonly note?: string;
}

interface OperationAccess {
  readonly operation: DocumentOperation;
  readonly kind: "allowed" | "conditional" | "denied";
  readonly conditions: readonly PolicyRule[];
  readonly denies: readonly PolicyRule[];
  readonly reason: string;
}

interface PropertyRow {
  readonly name: string;
  readonly type: string;
  readonly required: boolean;
  readonly primaryKey: boolean;
  readonly description: string;
}

/**
 * The only key material these pages may carry. Anything that is not a public
 * project key -- a service credential, a secret reference, an empty string --
 * is refused here, before it can reach a snippet.
 */
function publicKeyMaterial(value: string): string {
  if (!value.startsWith(PUBLIC_KEY_PREFIX) || value.includes(SERVICE_KEY_PREFIX)) {
    throw new Error(
      "API documentation carries public project keys only; the key on file was refused.",
    );
  }
  return value;
}

export function ApiDocsScreen({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [docs, setDocs] = useState<GeneratedDocs | null>(null);
  const [quickstart, setQuickstart] = useState<QuickstartClient>("curl");
  // The latest generation wins: a slow read from a previous environment or an
  // earlier regenerate never lands on top of a newer one.
  const generation = useRef(0);

  const regenerate = useCallback(async () => {
    const token = generation.current + 1;
    generation.current = token;
    setDocs(null);
    const collections = async (): Promise<CollectionReference[]> => {
      const items = await client.listCollections(projectId, environmentId);
      return Promise.all(
        items.map(async (collection) => {
          const [indexes, policy] = await Promise.all([
            settle(client.listCollectionIndexes(projectId, environmentId, collection.id)),
            settle(client.getActiveCollectionPolicy(projectId, environmentId, collection.id)),
          ]);
          return { collection, indexes, policy };
        }),
      );
    };
    const connect = async (): Promise<ConnectReference> => {
      const metadata = await client.getConnectMetadata(projectId, environmentId);
      return {
        apiUrl: metadata.publicEndpoint,
        publicKeyId: metadata.publicKeyId,
        publicKey: metadata.publicKey === "" ? null : publicKeyMaterial(metadata.publicKey),
        rxdbClientRange: metadata.rxdbClientRange,
      };
    };
    const [nextCollections, nextFunctions, nextBuckets, nextAuth, nextConnect] = await Promise.all([
      settle(collections()),
      settle(client.listFunctions(projectId, environmentId)),
      settle(client.listStorageBuckets(projectId, environmentId)),
      settle(client.getAuthSettings(projectId, environmentId)),
      settle(connect()),
    ]);
    if (generation.current !== token) return;
    setDocs({
      generatedAt: new Date().toISOString(),
      collections: nextCollections,
      functions: nextFunctions,
      buckets: nextBuckets,
      auth: nextAuth,
      connect: nextConnect,
    });
  }, [client, environmentId, projectId]);
  useEffect(() => {
    void regenerate();
  }, [regenerate]);

  const context = docs === null ? null : contextFrom(projectId, environmentId, docs);
  return (
    <section className="api-docs" aria-labelledby="api-docs-title">
      <div className="section-heading">
        <div>
          <p className="eyebrow">API documentation</p>
          <h1 id="api-docs-title">This environment's API</h1>
          <p>
            Generated in the console from the environment's collections, schemas, indexes, active
            policies, functions, buckets, and public connection metadata. Nothing is stored: refresh
            to regenerate from the current state.
          </p>
        </div>
        <button type="button" className="secondary" onClick={() => void regenerate()}>
          Regenerate
        </button>
      </div>
      {docs === null || context === null ? (
        <p aria-busy="true">Generating documentation from the environment…</p>
      ) : (
        <>
          <p className="api-docs-generated" role="status">
            Generated from the environment at{" "}
            <time dateTime={docs.generatedAt}>{docs.generatedAt}</time>
          </p>
          <nav className="api-docs-toc" aria-label="Documentation sections">
            {[
              ["overview", "Overview"],
              ["auth", "Auth"],
              ["collections", "Collections"],
              ["functions", "Functions"],
              ["storage", "Storage"],
              ["quickstarts", "Quickstarts"],
            ].map(([id, label]) => (
              <a
                key={id}
                href={`#api-docs-${id}`}
                onClick={(event) => {
                  event.preventDefault();
                  document.getElementById(`api-docs-${id}`)?.scrollIntoView();
                }}
              >
                {label}
              </a>
            ))}
          </nav>
          <OverviewSection docs={docs} context={context} />
          <AuthSection docs={docs} context={context} />
          <CollectionsSection docs={docs} context={context} />
          <FunctionsSection docs={docs} context={context} />
          <StorageSection docs={docs} context={context} />
          <QuickstartsSection
            docs={docs}
            context={context}
            selected={quickstart}
            onSelect={setQuickstart}
          />
        </>
      )}
    </section>
  );
}

function contextFrom(projectId: string, environmentId: string, docs: GeneratedDocs): DocsContext {
  const connect = docs.connect.status === "ready" ? docs.connect.value : null;
  const auth = docs.auth.status === "ready" ? docs.auth.value : null;
  const provider = auth?.providers.find((item) => item.enabled) ?? auth?.providers[0];
  return {
    projectId,
    environmentId,
    apiUrl: connect?.apiUrl ?? API_URL_PLACEHOLDER,
    publicKeyId: connect?.publicKeyId ?? null,
    // Whatever is substituted into a snippet passes the public-key guard, the
    // placeholder included: no other material can be spliced in here.
    keyMaterial: publicKeyMaterial(connect?.publicKey ?? PUBLIC_KEY_PLACEHOLDER),
    providerName: provider?.name ?? "github",
    redirectUrl: auth?.redirectUrls[0] ?? SAMPLE_REDIRECT_URL,
    magicLinksEnabled: auth === null ? null : auth.magicLinks.enabled,
  };
}

// --- Sections -------------------------------------------------------------

function OverviewSection({
  docs,
  context,
}: {
  readonly docs: GeneratedDocs;
  readonly context: DocsContext;
}) {
  return (
    <DocsSection id="overview" title="Overview">
      <PartView part={docs.connect} label="Connection metadata">
        {(connect) => (
          <dl className="definition-grid">
            <div>
              <dt>API URL</dt>
              <dd>
                <code>{connect.apiUrl}</code>
              </dd>
            </div>
            <div>
              <dt>Public key ID</dt>
              <dd>
                <code>{connect.publicKeyId}</code>
              </dd>
            </div>
            <div>
              <dt>Project</dt>
              <dd>
                <code>{context.projectId}</code>
              </dd>
            </div>
            <div>
              <dt>Environment</dt>
              <dd>
                <code>{context.environmentId}</code>
              </dd>
            </div>
            <div>
              <dt>Supported RxDB client</dt>
              <dd>
                <code>{connect.rxdbClientRange}</code>
              </dd>
            </div>
          </dl>
        )}
      </PartView>
      {docs.connect.status === "ready" && docs.connect.value.publicKey === null ? (
        <p className="notice warning">
          A public key's value is shown once, when it is issued or rotated on the Credentials page.
          The examples below use the placeholder <code>{PUBLIC_KEY_PLACEHOLDER}</code> where the
          value of <code>{docs.connect.value.publicKeyId}</code> belongs.
        </p>
      ) : null}
      <h3>Headers</h3>
      <table className="api-docs-headers">
        <thead>
          <tr>
            <th scope="col">Header</th>
            <th scope="col">Carries</th>
            <th scope="col">Where</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <th scope="row">
              <code>X-Mako-Key</code>
            </th>
            <td>The public project key. It identifies and meters a client and grants nothing.</td>
            <td>
              Sign-up, sign-in, refresh, provider start and exchange, magic links, JWKS, and
              replication alongside the bearer token.
            </td>
          </tr>
          <tr>
            <th scope="row">
              <code>Authorization: Bearer</code>
            </th>
            <td>
              An application user's access token from sign-in. Every document, replication, and
              storage request is evaluated under the collection's or bucket's policy as that user.
            </td>
            <td>
              Documents, replication, storage, sign-out, current user, and protected functions.
            </td>
          </tr>
          <tr>
            <th scope="row">
              <code>Idempotency-Key</code>
            </th>
            <td>A stable key per mutation so a retried timeout cannot apply twice.</td>
            <td>Document mutations and replication pushes.</td>
          </tr>
        </tbody>
      </table>
      <p className="api-docs-service-note">
        Service credentials (<code>X-Mako-Service-Key</code>) are not documented here and never
        belong in browser or mobile code.
      </p>
    </DocsSection>
  );
}

function AuthSection({
  docs,
  context,
}: {
  readonly docs: GeneratedDocs;
  readonly context: DocsContext;
}) {
  const auth = docs.auth.status === "ready" ? docs.auth.value : null;
  return (
    <DocsSection id="auth" title="Auth">
      <p>
        Application users belong to this environment alone. Password sign-up and sign-in take the
        public key; a session's access token is a short-lived JWT verified against the environment's
        JWKS, and the refresh token rotates it.
      </p>
      {docs.auth.status === "unavailable" ? (
        <PartNotice label="Sign-in settings" failure={docs.auth.failure} />
      ) : auth === null ? null : (
        <p>
          {auth.providers.length === 0
            ? "No external providers are configured; "
            : `Providers configured: ${auth.providers
                .map((provider) => `${provider.name}${provider.enabled ? "" : " (disabled)"}`)
                .join(", ")}; `}
          magic links are {auth.magicLinks.enabled ? "enabled" : "disabled"}
          {auth.redirectUrls.length === 0
            ? "; no redirect URL is registered, so provider and magic-link examples use a placeholder."
            : `; registered redirects: ${auth.redirectUrls.join(", ")}.`}
        </p>
      )}
      {authExamples(context).map((example) => (
        <ExampleView key={example.id} example={example} />
      ))}
    </DocsSection>
  );
}

function CollectionsSection({
  docs,
  context,
}: {
  readonly docs: GeneratedDocs;
  readonly context: DocsContext;
}) {
  return (
    <DocsSection id="collections" title="Collections">
      <PartView part={docs.collections} label="Collections">
        {(items) =>
          items.length === 0 ? (
            <p>No collections exist in this environment yet; create one to document it here.</p>
          ) : (
            items.map((item) => (
              <CollectionReferenceView key={item.collection.id} item={item} context={context} />
            ))
          )
        }
      </PartView>
    </DocsSection>
  );
}

function CollectionReferenceView({
  item,
  context,
}: {
  readonly item: CollectionReference;
  readonly context: DocsContext;
}) {
  const { collection } = item;
  const rows = propertyRows(collection);
  const sample = sampleDocument(collection);
  const primaryKeyField = primaryKeyFieldOf(collection);
  const documentId = String(sample[primaryKeyField] ?? `${collection.id}-1`);
  const indexes = item.indexes.status === "ready" ? item.indexes.value : [];
  const examples = collectionExamples(context, collection, sample, documentId, indexes);
  return (
    <article
      className="api-docs-collection"
      data-collection-id={collection.id}
      aria-labelledby={`api-docs-collection-${collection.id}`}
    >
      <h3 id={`api-docs-collection-${collection.id}`}>
        <code>{collection.id}</code>
      </h3>
      <p>
        Schema version {collection.schemaVersion} · primary key <code>{primaryKeyField}</code>
        {collection.primaryKey.kind === "composite"
          ? ` (composite of ${collection.primaryKey.fields.join(", ")})`
          : ""}{" "}
        · {humanize(collection.state)} · {humanize(collection.compatibility)}
      </p>
      <h4>Document shape</h4>
      {rows.length === 0 ? (
        <p>
          The schema declares no properties: documents are free-form objects keyed by{" "}
          <code>{primaryKeyField}</code>.
        </p>
      ) : (
        <div className="table-scroll">
          <table className="api-docs-shape">
            <thead>
              <tr>
                <th scope="col">Property</th>
                <th scope="col">Type</th>
                <th scope="col">Required</th>
                <th scope="col">Description</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.name} data-property={row.name}>
                  <th scope="row">
                    <code>{row.name}</code>
                    {row.primaryKey ? <small> primary key</small> : null}
                  </th>
                  <td>
                    <code>{row.type}</code>
                  </td>
                  <td>{row.required ? "required" : "optional"}</td>
                  <td>{row.description}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <h4>Indexes</h4>
      <PartView part={item.indexes} label="Indexes">
        {(list) =>
          list.length === 0 ? (
            <p>No secondary indexes; queries can predicate on the primary key only.</p>
          ) : (
            <ul className="api-docs-indexes">
              {list.map((index) => (
                <li key={`${index.name}/${index.version}`}>
                  <code>{index.name}</code> v{index.version} · {index.kind.replaceAll("_", " ")} ·{" "}
                  {humanize(index.state)} ·{" "}
                  {index.fields
                    .map((field) => `${field.path} ${field.direction === "ascending" ? "↑" : "↓"}`)
                    .join(", ")}
                </li>
              ))}
            </ul>
          )
        }
      </PartView>
      <h4>Allowed operations</h4>
      <PartView part={item.policy} label="Active policy">
        {(policy) => <OperationsView policy={policy} />}
      </PartView>
      <h4>Example requests</h4>
      {examples.map((example) => (
        <ExampleView key={example.id} example={example} />
      ))}
    </article>
  );
}

function OperationsView({ policy }: { readonly policy: ActivePolicy }) {
  const access = deriveOperations(policy);
  return (
    <>
      <p className="api-docs-derivation">
        {policy.policy === undefined
          ? "No policy is active on this collection, so every operation is denied. "
          : `Derived from active policy version ${policy.policy.version} (authorization epoch ${policy.authorizationEpoch}): `}
        an allow rule whose expression is <code>true</code> makes an operation allowed; allow rules
        with any other expression make it conditional on that expression; an operation no allow rule
        names is denied. Deny rules are listed because a matching deny overrides every allow.
      </p>
      <div className="api-docs-operations">
        {access.map((entry) => (
          <article
            key={entry.operation}
            className="api-docs-operation"
            data-operation={entry.operation}
            data-access={entry.kind}
          >
            <div className="section-heading">
              <h5>{humanize(entry.operation)}</h5>
              <span
                className={`status-badge ${entry.kind === "allowed" ? "success" : entry.kind === "denied" ? "error" : ""}`}
              >
                {humanize(entry.kind)}
              </span>
            </div>
            <small>{entry.reason}</small>
            {entry.conditions.length > 0 ? (
              <ul>
                {entry.conditions.map((rule) => (
                  <li key={rule.id}>
                    <code>{rule.expression}</code> <small>({rule.id})</small>
                  </li>
                ))}
              </ul>
            ) : null}
            {entry.denies.length > 0 ? (
              <ul className="api-docs-denies">
                {entry.denies.map((rule) => (
                  <li key={rule.id}>
                    Denied when <code>{rule.expression}</code> <small>({rule.id})</small>
                  </li>
                ))}
              </ul>
            ) : null}
          </article>
        ))}
      </div>
    </>
  );
}

function FunctionsSection({
  docs,
  context,
}: {
  readonly docs: GeneratedDocs;
  readonly context: DocsContext;
}) {
  return (
    <DocsSection id="functions" title="Functions">
      <p>
        Functions are invoked through the project reference, which encodes the environment:{" "}
        <code>{`${context.apiUrl}/${context.projectId}--${context.environmentId}/functions/v1/{name}`}</code>
        . Only a function with an active version answers.
      </p>
      <PartView part={docs.functions} label="Functions">
        {(items) =>
          items.length === 0 ? (
            <p>No functions exist in this environment.</p>
          ) : (
            items.map((item) => {
              const route = functionRoute(context, item.name);
              return (
                <article
                  key={item.name}
                  className="api-docs-function"
                  data-function-name={item.name}
                >
                  <h3>
                    <code>{item.name}</code>
                  </h3>
                  <dl className="definition-grid">
                    <div>
                      <dt>Route</dt>
                      <dd>
                        <code>{route}</code>
                      </dd>
                    </div>
                    <div>
                      <dt>Active version</dt>
                      <dd>
                        {item.activeVersion === null
                          ? "None deployed; the route does not answer yet"
                          : `v${item.activeVersion}`}
                      </dd>
                    </div>
                    <div>
                      <dt>Caller</dt>
                      <dd>
                        {item.configuration.verifyJwt
                          ? "Application session required (JWT verified)"
                          : "Public: no session is verified"}
                      </dd>
                    </div>
                    <div>
                      <dt>Regions</dt>
                      <dd>{item.configuration.regions.join(", ") || "none"}</dd>
                    </div>
                  </dl>
                  <ExampleView example={functionExample(context, item)} />
                </article>
              );
            })
          )
        }
      </PartView>
    </DocsSection>
  );
}

function StorageSection({
  docs,
  context,
}: {
  readonly docs: GeneratedDocs;
  readonly context: DocsContext;
}) {
  return (
    <DocsSection id="storage" title="Storage">
      <PartView part={docs.buckets} label="Buckets">
        {(items) =>
          items.length === 0 ? (
            <p>No buckets exist in this environment.</p>
          ) : (
            items.map((bucket) => (
              <article key={bucket.id} className="api-docs-bucket" data-bucket-id={bucket.id}>
                <h3>
                  <code>{bucket.id}</code>
                </h3>
                <p>
                  {bucket.access === "public"
                    ? "Public: anyone may download and list; writes need a session under the bucket's rules."
                    : "Policy: every read and write is evaluated under the bucket's rules as the signed-in user."}{" "}
                  Objects up to {bucket.maxObjectBytes.toLocaleString()} bytes
                  {bucket.allowedContentTypes.length === 0
                    ? ", any content type."
                    : `; content types ${bucket.allowedContentTypes.join(", ")}.`}
                </p>
                {bucketExamples(context, bucket).map((example) => (
                  <ExampleView key={example.id} example={example} />
                ))}
              </article>
            ))
          )
        }
      </PartView>
    </DocsSection>
  );
}

function QuickstartsSection({
  docs,
  context,
  selected,
  onSelect,
}: {
  readonly docs: GeneratedDocs;
  readonly context: DocsContext;
  readonly selected: QuickstartClient;
  readonly onSelect: (client: QuickstartClient) => void;
}) {
  const first =
    docs.collections.status === "ready" ? (docs.collections.value[0]?.collection ?? null) : null;
  const firstFunction =
    docs.functions.status === "ready" ? (docs.functions.value[0] ?? null) : null;
  const snippet =
    selected === "curl"
      ? curlQuickstart(context, first, firstFunction)
      : selected === "javascript"
        ? fetchQuickstart(context, first)
        : rxdbQuickstart(context, first);
  return (
    <DocsSection id="quickstarts" title="Quickstarts">
      <p className="api-docs-service-note">
        Each quickstart carries this environment's API URL and public key
        {context.publicKeyId === null ? "" : ` (${context.publicKeyId})`}. Service credentials never
        appear here.
      </p>
      {first === null ? (
        <p>
          No collection exists yet, so the snippets use a placeholder collection <code>todos</code>.
        </p>
      ) : null}
      <div className="tab-list" role="tablist" aria-label="Quickstart client">
        {QUICKSTART_CLIENTS.map((client) => (
          <button
            key={client.id}
            type="button"
            role="tab"
            id={`api-docs-quickstart-tab-${client.id}`}
            aria-selected={selected === client.id}
            aria-controls="api-docs-quickstart-panel"
            className={selected === client.id ? "active" : "secondary"}
            onClick={() => onSelect(client.id)}
          >
            {client.label}
          </button>
        ))}
      </div>
      <div
        id="api-docs-quickstart-panel"
        role="tabpanel"
        aria-labelledby={`api-docs-quickstart-tab-${selected}`}
      >
        <CodeBlock text={snippet} label={`Copy ${labelOf(selected)} quickstart`} />
      </div>
    </DocsSection>
  );
}

// --- Shared views -----------------------------------------------------------

function DocsSection({
  id,
  title,
  children,
}: {
  readonly id: string;
  readonly title: string;
  readonly children: ReactNode;
}) {
  return (
    <section className="panel" id={`api-docs-${id}`} aria-labelledby={`api-docs-${id}-title`}>
      <h2 id={`api-docs-${id}-title`}>{title}</h2>
      {children}
    </section>
  );
}

function PartView<T>({
  part,
  label,
  children,
}: {
  readonly part: Part<T>;
  readonly label: string;
  readonly children: (value: T) => ReactNode;
}) {
  if (part.status === "loading") return <p>Loading {label.toLowerCase()}…</p>;
  if (part.status === "unavailable") return <PartNotice label={label} failure={part.failure} />;
  return <>{children(part.value)}</>;
}

function PartNotice({
  label,
  failure,
}: {
  readonly label: string;
  readonly failure: ConsoleApiFailure;
}) {
  return (
    <div className="notice warning" role="status">
      <p>
        {label} could not be read: {failure.message} The other sections are unaffected; regenerate
        to try again.
      </p>
      <RequestId value={failure.requestId} />
    </div>
  );
}

function ExampleView({ example }: { readonly example: ExampleRequest }) {
  return (
    <article className="api-docs-example" data-example-id={example.id}>
      <h5>{example.title}</h5>
      <p>
        <span className={`api-docs-method method-${example.method.toLowerCase()}`}>
          {example.method}
        </span>{" "}
        <code className="api-docs-url">{example.url}</code>
      </p>
      {example.note === undefined ? null : <small>{example.note}</small>}
      <CodeBlock text={curlFor(example)} label={`Copy ${example.title} request`} />
      <details>
        <summary>Response</summary>
        <pre className="json-view">
          {typeof example.response === "string"
            ? example.response
            : JSON.stringify(example.response, null, 2)}
        </pre>
      </details>
    </article>
  );
}

function CodeBlock({ text, label }: { readonly text: string; readonly label: string }) {
  return (
    <div className="code-block">
      <button
        type="button"
        className="secondary copy-button"
        aria-label={label}
        onClick={() => void navigator.clipboard.writeText(text)}
      >
        Copy
      </button>
      <pre>{text}</pre>
    </div>
  );
}

// --- Derivations --------------------------------------------------------------

/**
 * How the operation table is derived from the active policy: an allow rule
 * whose expression is `true` makes an operation allowed; allow rules with any
 * other expression make it conditional on those expressions; an operation no
 * allow rule names is denied. Deny rules are reported alongside, because a
 * matching deny overrides every allow.
 */
function deriveOperations(policy: ActivePolicy): OperationAccess[] {
  const rules = policy.policy?.rules ?? [];
  return DOCUMENT_OPERATIONS.map((operation) => {
    const covering = rules.filter((rule) => rule.operations.includes(operation));
    const allows = covering.filter((rule) => rule.effect === "allow");
    const denies = covering.filter((rule) => rule.effect === "deny");
    if (policy.policy === undefined) {
      return {
        operation,
        kind: "denied",
        conditions: [],
        denies,
        reason: "No policy is active; the environment denies by default.",
      };
    }
    if (allows.some((rule) => rule.expression.trim() === "true")) {
      return {
        operation,
        kind: "allowed",
        conditions: [],
        denies,
        reason: "An allow rule with the expression true covers this operation.",
      };
    }
    if (allows.length > 0) {
      return {
        operation,
        kind: "conditional",
        conditions: allows,
        denies,
        reason: "Allowed only when one of these allow expressions holds.",
      };
    }
    return {
      operation,
      kind: "denied",
      conditions: [],
      denies,
      reason: "No allow rule names this operation.",
    };
  });
}

function primaryKeyFieldOf(collection: Collection): string {
  return collection.primaryKey.kind === "field"
    ? collection.primaryKey.field
    : collection.primaryKey.key;
}

function propertyRows(collection: Collection): PropertyRow[] {
  const properties = record(collection.jsonSchema.properties);
  if (properties === null) return [];
  const required = new Set(stringList(collection.jsonSchema.required));
  const primaryKey = primaryKeyFieldOf(collection);
  const rows = Object.entries(properties).map(([name, schema]) => {
    const definition = record(schema);
    return {
      name,
      type: typeLabel(definition),
      required: required.has(name) || name === primaryKey,
      primaryKey: name === primaryKey,
      description: typeof definition?.description === "string" ? definition.description : "",
    };
  });
  return rows.sort((left, right) => Number(right.primaryKey) - Number(left.primaryKey));
}

function typeLabel(schema: Record<string, unknown> | null): string {
  if (schema === null) return "any";
  const options = Array.isArray(schema.enum) ? schema.enum : null;
  if (options !== null && options.length > 0) {
    return `enum: ${options.map((option) => JSON.stringify(option)).join(" | ")}`;
  }
  const types = Array.isArray(schema.type)
    ? stringList(schema.type)
    : typeof schema.type === "string"
      ? [schema.type]
      : [];
  const base = types.length === 0 ? "any" : types.join(" | ");
  if (types.includes("array")) {
    return `array<${typeLabel(record(schema.items))}>`;
  }
  return typeof schema.format === "string" ? `${base} (${schema.format})` : base;
}

/** A document that satisfies the schema's declared shape, built by type. */
function sampleDocument(collection: Collection): Record<string, unknown> {
  const sample = record(sampleValue(collection.jsonSchema, collection.id, 0)) ?? {};
  const primaryKey = primaryKeyFieldOf(collection);
  if (collection.primaryKey.kind === "field") {
    sample[primaryKey] = `${collection.id}-1`;
  } else {
    for (const field of collection.primaryKey.fields) {
      sample[field] = typeof sample[field] === "string" ? sample[field] : `${field}-1`;
    }
    sample[primaryKey] = collection.primaryKey.fields
      .map((field) => String(sample[field]))
      .join("|");
  }
  return sample;
}

function sampleValue(schema: unknown, name: string, depth: number): unknown {
  const definition = record(schema);
  if (definition === null) return name;
  if (Array.isArray(definition.examples) && definition.examples.length > 0) {
    return definition.examples[0];
  }
  if ("const" in definition) return definition.const;
  if ("default" in definition) return definition.default;
  if (Array.isArray(definition.enum) && definition.enum.length > 0) return definition.enum[0];
  const types = Array.isArray(definition.type)
    ? stringList(definition.type)
    : typeof definition.type === "string"
      ? [definition.type]
      : [];
  const type =
    types.find((item) => item !== "null") ?? (record(definition.properties) ? "object" : "string");
  switch (type) {
    case "string": {
      const format = definition.format;
      const value =
        format === "date-time"
          ? SAMPLE_TIME
          : format === "date"
            ? SAMPLE_TIME.slice(0, 10)
            : format === "email"
              ? "user@example.com"
              : format === "uri"
                ? "https://example.com"
                : format === "uuid"
                  ? "00000000-0000-4000-8000-000000000000"
                  : `${name}-example`;
      const maxLength = typeof definition.maxLength === "number" ? definition.maxLength : null;
      return maxLength === null ? value : value.slice(0, Math.max(1, maxLength));
    }
    case "integer":
    case "number": {
      const minimum = typeof definition.minimum === "number" ? definition.minimum : 0;
      return type === "integer" ? Math.ceil(minimum) : minimum;
    }
    case "boolean":
      return true;
    case "null":
      return null;
    case "array":
      return depth >= 3 ? [] : [sampleValue(definition.items, name, depth + 1)];
    case "object": {
      const properties = record(definition.properties);
      if (properties === null || depth >= 3) return {};
      return Object.fromEntries(
        Object.entries(properties).map(([key, value]) => [key, sampleValue(value, key, depth + 1)]),
      );
    }
    default:
      return name;
  }
}

// --- Examples -----------------------------------------------------------------

function authBase(context: DocsContext): string {
  return `${context.apiUrl}/v1/projects/${context.projectId}/environments/${context.environmentId}/auth`;
}

function collectionBase(context: DocsContext, collectionId: string): string {
  return `${context.apiUrl}/v1/projects/${context.projectId}/environments/${context.environmentId}/collections/${collectionId}`;
}

function storageBase(context: DocsContext, bucketId: string): string {
  return `${context.apiUrl}/v1/projects/${context.projectId}/environments/${context.environmentId}/storage/${bucketId}/objects`;
}

function functionRoute(context: DocsContext, name: string): string {
  return `${context.apiUrl}/${context.projectId}--${context.environmentId}/functions/v1/${name}`;
}

function keyHeader(context: DocsContext): readonly [string, string] {
  return ["X-Mako-Key", publicKeyMaterial(context.keyMaterial)];
}

const BEARER_HEADER = ["Authorization", "Bearer $MAKO_ACCESS_TOKEN"] as const;
const JSON_HEADER = ["Content-Type", "application/json"] as const;
const IDEMPOTENCY_HEADER = ["Idempotency-Key", "$(uuidgen)"] as const;

function sessionResponse() {
  return {
    accessToken: "<access token: a short-lived JWT>",
    refreshToken: "<refresh token: rotate it with POST …/auth/token>",
    expiresIn: 900,
    user: {
      id: SAMPLE_USER_ID,
      email: "user@example.com",
      status: "active",
      authorizationEpoch: 1,
    },
  };
}

function authExamples(context: DocsContext): ExampleRequest[] {
  const base = authBase(context);
  const key = keyHeader(context);
  return [
    {
      id: "auth-signup",
      title: "Sign up with a password",
      method: "POST",
      url: `${base}/signup`,
      headers: [key, JSON_HEADER],
      body: { email: "user@example.com", password: "correct horse battery staple" },
      response: { accepted: true },
      note: "Accepted whether or not the address exists; the account activates when the emailed verification is completed.",
    },
    {
      id: "auth-signin",
      title: "Sign in with a password",
      method: "POST",
      url: `${base}/signin`,
      headers: [key, JSON_HEADER],
      body: { email: "user@example.com", password: "correct horse battery staple" },
      response: sessionResponse(),
    },
    {
      id: "auth-refresh",
      title: "Refresh the session",
      method: "POST",
      url: `${base}/token`,
      headers: [key, JSON_HEADER],
      body: { refreshToken: "$MAKO_REFRESH_TOKEN" },
      response: sessionResponse(),
      note: "The refresh token rotates: keep the one in the response and discard the one you sent.",
    },
    {
      id: "auth-user",
      title: "Current user",
      method: "GET",
      url: `${base}/user`,
      headers: [BEARER_HEADER],
      response: {
        id: SAMPLE_USER_ID,
        email: "user@example.com",
        status: "active",
        authorizationEpoch: 1,
      },
    },
    {
      id: "auth-signout",
      title: "Sign out",
      method: "POST",
      url: `${base}/signout`,
      headers: [BEARER_HEADER],
      response: "204 No Content: the session is revoked.",
    },
    {
      id: "auth-provider-start",
      title: `Provider sign-in, step 1: start (${context.providerName})`,
      method: "POST",
      url: `${base}/providers/${context.providerName}/start`,
      headers: [key, JSON_HEADER],
      body: { redirectUrl: context.redirectUrl },
      response: {
        authorizationUrl: "https://<provider>/authorize?…&state=<signed state>",
        provider: context.providerName,
      },
      note: "Navigate the browser to authorizationUrl. The redirect must be registered for this environment.",
    },
    {
      id: "auth-provider-callback",
      title: "Provider sign-in, step 2: the provider's callback",
      method: "GET",
      url: `${base}/providers/${context.providerName}/callback?state=<signed state>&code=<provider code>`,
      headers: [],
      response: `302 Location: ${context.redirectUrl}#code=<one-time code>`,
      note: "Reached by browser navigation from the provider, so it carries no project key; a refusal is delivered the same way with an error in the fragment.",
    },
    {
      id: "auth-provider-exchange",
      title: "Provider sign-in, step 3: exchange the one-time code",
      method: "POST",
      url: `${base}/providers/exchange`,
      headers: [key, JSON_HEADER],
      body: { code: "<one-time code from the redirect fragment>" },
      response: sessionResponse(),
    },
    {
      id: "auth-magic-link",
      title: "Request a magic link",
      method: "POST",
      url: `${base}/magic-link`,
      headers: [key, JSON_HEADER],
      body: { email: "user@example.com", redirectUrl: context.redirectUrl },
      response: { accepted: true },
      note:
        context.magicLinksEnabled === false
          ? "Magic links are disabled for this environment; enable them under Auth providers before this answers with a link."
          : "Accepted whether or not the address is registered; an unknown address is signed up and activated by the link.",
    },
    {
      id: "auth-magic-link-redeem",
      title: "Redeem a magic link",
      method: "POST",
      url: `${base}/magic-link/redeem`,
      headers: [key, JSON_HEADER],
      body: { token: "<token from the emailed link>" },
      response: sessionResponse(),
    },
  ];
}

function collectionExamples(
  context: DocsContext,
  collection: Collection,
  sample: Record<string, unknown>,
  documentId: string,
  indexes: readonly CollectionIndex[],
): ExampleRequest[] {
  const base = collectionBase(context, collection.id);
  const primaryKey = primaryKeyFieldOf(collection);
  const document = `${base}/documents/${encodeURIComponent(documentId)}`;
  const indexedField = indexes.find((index) => index.state === "active")?.fields[0]?.path;
  const queryField = indexedField ?? primaryKey;
  const documentRecord = {
    primaryKey: documentId,
    schemaVersion: collection.schemaVersion,
    revision: SAMPLE_REVISION,
    commitPosition: 1,
    _deleted: false,
    body: sample,
  };
  const mutationId = (operation: string) => `${collection.id}-${operation}-0001`;
  return [
    {
      id: `${collection.id}-create`,
      title: "Create a document",
      method: "POST",
      url: document,
      headers: [BEARER_HEADER, IDEMPOTENCY_HEADER, JSON_HEADER],
      body: {
        mutationId: mutationId("create"),
        operation: "create",
        expectedRevision: null,
        schemaVersion: collection.schemaVersion,
        body: sample,
      },
      response: {
        mutationId: mutationId("create"),
        status: "applied",
        document: documentRecord,
        currentRevision: SAMPLE_REVISION,
      },
      note: "The document id in the path is its primary key; the body is validated against the schema and the create rule.",
    },
    {
      id: `${collection.id}-read`,
      title: "Read a document",
      method: "GET",
      url: document,
      headers: [BEARER_HEADER],
      response: documentRecord,
    },
    {
      id: `${collection.id}-query`,
      title: "Query documents",
      method: "POST",
      url: `${base}/documents/query`,
      headers: [BEARER_HEADER, JSON_HEADER],
      body: {
        predicates: [{ field: queryField, operator: "eq", value: sample[queryField] ?? null }],
        sort: [{ field: queryField, direction: "asc" }],
        cursor: null,
        limit: 50,
      },
      response: { documents: [documentRecord], nextCursor: null },
      note:
        indexedField === undefined
          ? "Predicates and sorts must be served by an active index or the primary key."
          : `Predicates on ${indexedField} are served by the active index; other fields need an index of their own.`,
    },
    {
      id: `${collection.id}-update`,
      title: "Update a document",
      method: "POST",
      url: document,
      headers: [BEARER_HEADER, IDEMPOTENCY_HEADER, JSON_HEADER],
      body: {
        mutationId: mutationId("update"),
        operation: "update",
        expectedRevision: SAMPLE_REVISION,
        schemaVersion: collection.schemaVersion,
        body: sample,
      },
      response: {
        mutationId: mutationId("update"),
        status: "applied",
        document: { ...documentRecord, revision: "2-3c1d8a90", commitPosition: 2 },
        currentRevision: "2-3c1d8a90",
      },
      note: "expectedRevision is the revision you read; a stale one answers with status conflict and the current revision.",
    },
    {
      id: `${collection.id}-delete`,
      title: "Delete a document",
      method: "POST",
      url: document,
      headers: [BEARER_HEADER, IDEMPOTENCY_HEADER, JSON_HEADER],
      body: {
        mutationId: mutationId("delete"),
        operation: "delete",
        expectedRevision: "2-3c1d8a90",
        schemaVersion: collection.schemaVersion,
        body: sample,
      },
      response: {
        mutationId: mutationId("delete"),
        status: "applied",
        document: { ...documentRecord, revision: "3-7b0e2f41", commitPosition: 3, _deleted: true },
        currentRevision: "3-7b0e2f41",
      },
    },
    {
      id: `${collection.id}-pull`,
      title: "RxDB replication: pull",
      method: "POST",
      url: `${base}/replication/pull`,
      headers: [BEARER_HEADER, keyHeader(context), JSON_HEADER],
      body: { checkpoint: null, schemaVersion: collection.schemaVersion, batchSize: 100 },
      response: { documents: [sample], checkpoint: "<signed opaque checkpoint>" },
      note: "Send the last checkpoint back to continue; a null checkpoint starts from the beginning of what the user may read.",
    },
    {
      id: `${collection.id}-push`,
      title: "RxDB replication: push",
      method: "POST",
      url: `${base}/replication/push`,
      headers: [BEARER_HEADER, keyHeader(context), IDEMPOTENCY_HEADER, JSON_HEADER],
      body: {
        schemaVersion: collection.schemaVersion,
        rows: [
          { mutationId: mutationId("push"), assumedMasterState: null, newDocumentState: sample },
        ],
      },
      response: { outcomes: [{ mutationId: mutationId("push"), status: "accepted" }] },
      note: "Each row answers accepted, conflict (with the master state when readable), or denied.",
    },
    {
      id: `${collection.id}-stream`,
      title: "RxDB replication: live stream",
      method: "GET",
      url: `${base}/replication/stream?schemaVersion=${collection.schemaVersion}`,
      headers: [BEARER_HEADER, keyHeader(context), ["Accept", "text/event-stream"]],
      response:
        "event: documents / checkpoint / heartbeat / resync, as server-sent events; resume with ?cursor=<Last-Event-ID>.",
    },
  ];
}

function functionExample(context: DocsContext, item: EdgeFunction): ExampleRequest {
  return {
    id: `function-${item.name}`,
    title: `Invoke ${item.name}`,
    method: "POST",
    url: functionRoute(context, item.name),
    headers: item.configuration.verifyJwt ? [BEARER_HEADER, JSON_HEADER] : [JSON_HEADER],
    body: { example: true },
    response:
      item.activeVersion === null
        ? "No version is active, so the route does not answer until one is promoted."
        : `The response of ${item.name} v${item.activeVersion}.`,
  };
}

function bucketExamples(context: DocsContext, bucket: StorageBucket): ExampleRequest[] {
  const base = storageBase(context, bucket.id);
  const contentType = bucket.allowedContentTypes[0]?.replace("/*", "/png") ?? "image/png";
  const object = {
    bucketId: bucket.id,
    path: "uploads/example.png",
    contentType,
    sizeBytes: 12_345,
    ownerId: SAMPLE_USER_ID,
    digest: "sha256:<plaintext digest>",
    storedDigest: "sha256:<stored digest>",
    createdAtUnixSeconds: 1_767_225_600,
    updatedAtUnixSeconds: 1_767_225_600,
  };
  const readHeaders = bucket.access === "public" ? [] : [BEARER_HEADER];
  return [
    {
      id: `bucket-${bucket.id}-upload`,
      title: "Upload an object",
      method: "PUT",
      url: `${base}/uploads/example.png`,
      headers: [BEARER_HEADER, ["Content-Type", contentType]],
      rawData: "--data-binary @./example.png",
      response: object,
      note: "The body is the object; a new path is a create and an existing one an update under the bucket's rules.",
    },
    {
      id: `bucket-${bucket.id}-download`,
      title: "Download an object",
      method: "GET",
      url: `${base}/uploads/example.png`,
      headers: readHeaders,
      response: `The bytes with their Content-Type and an ETag of the plaintext digest.${bucket.access === "public" ? " A public bucket answers without a credential." : ""}`,
    },
    {
      id: `bucket-${bucket.id}-list`,
      title: "List objects",
      method: "GET",
      url: `${base}?prefix=uploads/&limit=100`,
      headers: readHeaders,
      response: { items: [object], nextCursor: null },
    },
    {
      id: `bucket-${bucket.id}-delete`,
      title: "Delete an object",
      method: "DELETE",
      url: `${base}/uploads/example.png`,
      headers: [BEARER_HEADER],
      response: "204 No Content.",
    },
  ];
}

function curlFor(example: ExampleRequest): string {
  const lines = [`curl -X ${example.method} "${example.url}"`];
  for (const [name, value] of example.headers) {
    lines.push(`  -H "${name}: ${value}"`);
  }
  if (example.rawData !== undefined) {
    lines.push(`  ${example.rawData}`);
  } else if (example.body !== undefined) {
    lines.push(`  -d '${JSON.stringify(example.body, null, 2)}'`);
  }
  return lines.join(" \\\n");
}

// --- Quickstarts --------------------------------------------------------------

function curlQuickstart(
  context: DocsContext,
  collection: Collection | null,
  edgeFunction: EdgeFunction | null,
): string {
  const collectionId = collection?.id ?? "todos";
  const schemaVersion = collection?.schemaVersion ?? 1;
  const queryField = collection === null ? "id" : primaryKeyFieldOf(collection);
  const lines = [
    `# ${context.environmentId} in ${context.projectId}`,
    `export MAKO_API_URL="${context.apiUrl}"`,
    `export MAKO_PUBLIC_KEY="${publicKeyMaterial(context.keyMaterial)}"${context.publicKeyId === null ? "" : `  # key id ${context.publicKeyId}`}`,
    `export MAKO_TENANT="projects/${context.projectId}/environments/${context.environmentId}"`,
    "",
    "# Sign in as an application user and keep the access token.",
    'MAKO_ACCESS_TOKEN=$(curl -sS -X POST "$MAKO_API_URL/v1/$MAKO_TENANT/auth/signin" \\',
    '  -H "X-Mako-Key: $MAKO_PUBLIC_KEY" -H "Content-Type: application/json" \\',
    `  -d '{"email":"user@example.com","password":"correct horse battery staple"}' | jq -r .accessToken)`,
    "",
    `# Query ${collectionId} (schema v${schemaVersion}) as that user.`,
    `curl -sS -X POST "$MAKO_API_URL/v1/$MAKO_TENANT/collections/${collectionId}/documents/query" \\`,
    '  -H "Authorization: Bearer $MAKO_ACCESS_TOKEN" -H "Content-Type: application/json" \\',
    `  -d '{"predicates":[],"sort":[{"field":"${queryField}","direction":"asc"}],"cursor":null,"limit":50}'`,
  ];
  if (edgeFunction !== null) {
    lines.push(
      "",
      `# Invoke the function ${edgeFunction.name}.`,
      `curl -sS -X POST "$MAKO_API_URL/${context.projectId}--${context.environmentId}/functions/v1/${edgeFunction.name}" \\`,
      edgeFunction.configuration.verifyJwt
        ? '  -H "Authorization: Bearer $MAKO_ACCESS_TOKEN" -H "Content-Type: application/json" \\'
        : '  -H "Content-Type: application/json" \\',
      `  -d '{"example":true}'`,
    );
  }
  return lines.join("\n");
}

function fetchQuickstart(context: DocsContext, collection: Collection | null): string {
  const collectionId = collection?.id ?? "todos";
  const schemaVersion = collection?.schemaVersion ?? 1;
  const queryField = collection === null ? "id" : primaryKeyFieldOf(collection);
  return `// ${context.environmentId} in ${context.projectId}
const API_URL = ${JSON.stringify(context.apiUrl)};
const PUBLIC_KEY = ${JSON.stringify(publicKeyMaterial(context.keyMaterial))};${context.publicKeyId === null ? "" : ` // key id ${context.publicKeyId}`}
const TENANT = "projects/${context.projectId}/environments/${context.environmentId}";

async function signIn(email, password) {
  const response = await fetch(\`\${API_URL}/v1/\${TENANT}/auth/signin\`, {
    method: "POST",
    headers: { "X-Mako-Key": PUBLIC_KEY, "Content-Type": "application/json" },
    body: JSON.stringify({ email, password }),
  });
  if (!response.ok) throw new Error(\`sign-in failed: \${response.status}\`);
  return response.json(); // { accessToken, refreshToken, expiresIn, user }
}

async function query${pascal(collectionId)}(accessToken) {
  const response = await fetch(
    \`\${API_URL}/v1/\${TENANT}/collections/${collectionId}/documents/query\`,
    {
      method: "POST",
      headers: { Authorization: \`Bearer \${accessToken}\`, "Content-Type": "application/json" },
      body: JSON.stringify({
        predicates: [],
        sort: [{ field: ${JSON.stringify(queryField)}, direction: "asc" }],
        cursor: null,
        limit: 50,
      }),
    },
  );
  if (!response.ok) throw new Error(\`query failed: \${response.status}\`);
  return response.json(); // { documents: [{ primaryKey, schemaVersion: ${schemaVersion}, revision, body }], nextCursor }
}

const session = await signIn("user@example.com", "correct horse battery staple");
console.log(await query${pascal(collectionId)}(session.accessToken));`;
}

function rxdbQuickstart(context: DocsContext, collection: Collection | null): string {
  const collectionId = collection?.id ?? "todos";
  const template = createMakoRxdbConnectTemplateV1({
    endpoint: context.apiUrl,
    projectId: context.projectId,
    environmentId: context.environmentId,
    collectionId,
    schemaVersion: collection?.schemaVersion ?? 1,
    publicProjectKey: publicKeyMaterial(context.keyMaterial),
  });
  const primaryKey = collection === null ? "id" : primaryKeyFieldOf(collection);
  return `// npm install @mako-cloud/rxdb rxdb@17 rxjs@7
${template}

import {
  MakoAuthClient,
  MakoLivePullStream,
  MakoReplicationSignals,
  createMakoPullOptions,
  createMakoPushOptions,
  type MakoCheckpoint,
} from "@mako-cloud/rxdb";
import { createRxDatabase } from "rxdb";
import { replicateRxCollection } from "rxdb/plugins/replication";
import { getRxStorageMemory } from "rxdb/plugins/storage-memory";

// Sign in as an application user; the session refreshes itself.
const auth = new MakoAuthClient(config);
await auth.signInWithPassword("user@example.com", "correct horse battery staple");

// The local schema mirrors ${collectionId} (schema v${collection?.schemaVersion ?? 1}); its primary key is ${JSON.stringify(primaryKey)}.
const database = await createRxDatabase({ name: "app", storage: getRxStorageMemory() });
const { ${pascalSafe(collectionId)} } = await database.addCollections({
  ${pascalSafe(collectionId)}: { schema: ${JSON.stringify(rxdbSchema(collection), null, 2).replaceAll("\n", "\n  ")} },
});

// Pull, push, and the live stream run under the signed-in user's policy.
const live = new MakoLivePullStream(config, auth);
const replication = replicateRxCollection<Record<string, unknown>, MakoCheckpoint>({
  replicationIdentifier: "mako-${collectionId}-v${collection?.schemaVersion ?? 1}",
  collection: ${pascalSafe(collectionId)},
  pull: createMakoPullOptions(config, auth, { stream$: live.stream$ }),
  push: createMakoPushOptions(config, auth),
  live: true,
});
const signals = new MakoReplicationSignals();
const subscriptions = signals.bind(replication);
live.start();
await replication.awaitInitialReplication();`;
}

function rxdbSchema(collection: Collection | null): Record<string, unknown> {
  if (collection === null) {
    return {
      version: 0,
      primaryKey: "id",
      type: "object",
      properties: { id: { type: "string", maxLength: 100 } },
      required: ["id"],
    };
  }
  const { properties, required } = collection.jsonSchema;
  return {
    version: 0,
    primaryKey: primaryKeyFieldOf(collection),
    type: "object",
    properties: record(properties) ?? {},
    required: stringList(required),
  };
}

// --- Helpers ------------------------------------------------------------------

function labelOf(client: QuickstartClient): string {
  return QUICKSTART_CLIENTS.find((item) => item.id === client)?.label ?? client;
}

function pascal(value: string): string {
  return value
    .split(/[^a-zA-Z0-9]+/u)
    .filter((part) => part !== "")
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join("");
}

/** A collection id as a JavaScript identifier: `todo-items` becomes `todoItems`. */
function pascalSafe(value: string): string {
  const name = pascal(value);
  return `${name.charAt(0).toLowerCase()}${name.slice(1)}` || "collection";
}

function humanize(value: string): string {
  const spaced = value.replace(/([a-z])([A-Z])/gu, "$1 $2").replaceAll("_", " ");
  return `${spaced.charAt(0).toUpperCase()}${spaced.slice(1)}`;
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function stringList(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}

/** Resolve to the part's outcome so one failing read never rejects the rest. */
function settle<T>(promise: Promise<T>): Promise<Part<T>> {
  return promise.then(
    (value) => ({ status: "ready", value }),
    (error: unknown) => ({ status: "unavailable", failure: failureFrom(error) }),
  );
}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof Error && !("requestId" in error)
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}
