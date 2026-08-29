import { type FormEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import type {
  Collection,
  ConnectMetadata,
  ConnectionCheck,
  DeveloperBackup,
  DeveloperRestore,
  Environment,
  Project,
  SyncSummary,
  WorkspaceDestination,
  WorkspaceSummary,
} from "@mako-cloud/management-sdk";
import { createMakoRxdbConnectTemplateV1 } from "@mako-cloud/rxdb";

import { AllowedOriginsSection } from "./allowed-origins.js";
import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { DataExplorer } from "./data-explorer.js";
import { useManagementClient } from "./management.js";

export type EnvironmentSection =
  | "overview"
  | "data"
  | "sync"
  | "policies"
  | "backups"
  | "connect"
  | "settings";

export function EnvironmentWorkspaceScreen({
  projectId,
  environmentId,
  section,
  navigate,
  explorerAdminEnabled,
  dataJobsEnabled,
  syncDetailsEnabled,
  restoreEnabled,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly section: EnvironmentSection;
  readonly navigate: (path: string, replace?: boolean) => void;
  readonly explorerAdminEnabled: boolean;
  readonly dataJobsEnabled: boolean;
  readonly syncDetailsEnabled: boolean;
  readonly restoreEnabled: boolean;
}) {
  let content: ReactNode;
  if (section === "data") {
    content = (
      <DataExplorer
        projectId={projectId}
        environmentId={environmentId}
        adminEnabled={explorerAdminEnabled}
        jobsEnabled={dataJobsEnabled}
        onCreateIndex={(collectionId, requiredIndex) =>
          navigate(
            `/projects/${projectId}/environments/${environmentId}/collections/${collectionId}?requiredIndex=${encodeURIComponent(requiredIndex)}`,
          )
        }
      />
    );
  } else if (section === "sync") {
    content = syncDetailsEnabled ? (
      <SyncDashboard projectId={projectId} environmentId={environmentId} />
    ) : (
      <DisabledWorkspaceFeature name="Detailed sync diagnostics" />
    );
  } else if (section === "backups") {
    content = (
      <BackupAndRestore
        projectId={projectId}
        environmentId={environmentId}
        restoreEnabled={restoreEnabled}
      />
    );
  } else if (section === "connect") {
    content = <ConnectPage projectId={projectId} environmentId={environmentId} />;
  } else if (section === "policies") {
    content = (
      <WorkspaceLinks
        title="Policies"
        description="Document policies are versioned per collection. Preview them as a selected application user before activation."
        links={[
          {
            label: "Open collections and policies",
            path: `/projects/${projectId}/environments/${environmentId}/collections`,
          },
          {
            label: "Open application users",
            path: `/projects/${projectId}/environments/${environmentId}/users`,
          },
        ]}
        navigate={navigate}
      />
    );
  } else if (section === "settings") {
    content = <EnvironmentSettings projectId={projectId} environmentId={environmentId} />;
  } else {
    content = (
      <WorkspaceOverview projectId={projectId} environmentId={environmentId} navigate={navigate} />
    );
  }
  return (
    <EnvironmentWorkspaceLayout
      projectId={projectId}
      environmentId={environmentId}
      section={section}
      navigate={navigate}
    >
      {content}
    </EnvironmentWorkspaceLayout>
  );
}

/// Destinations the console serves itself from signals the API already
/// exposes; the backend's navigation lists the areas it authorizes. Schedules
/// live on each function's page and custom domains on the project's, so
/// neither is listed here.
function consoleDestinations(projectId: string, environmentId: string): WorkspaceDestination[] {
  const base = `/projects/${projectId}/environments/${environmentId}`;
  return [
    { id: "storage", label: "Storage", path: `${base}/storage`, permitted: true },
    { id: "webhooks", label: "Webhooks", path: `${base}/webhooks`, permitted: true },
    {
      id: "auth-providers",
      label: "Auth providers",
      path: `${base}/auth-providers`,
      permitted: true,
    },
    {
      id: "email-templates",
      label: "Email templates",
      path: `${base}/email-templates`,
      permitted: true,
    },
    { id: "api-docs", label: "API docs", path: `${base}/api-docs`, permitted: true },
    { id: "logs", label: "Logs", path: `${base}/logs`, permitted: true },
    { id: "usage", label: "Usage", path: `${base}/usage`, permitted: true },
    { id: "activity", label: "Activity", path: `${base}/activity`, permitted: true },
  ];
}

export function EnvironmentWorkspaceLayout({
  projectId,
  environmentId,
  section,
  navigate,
  children,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly section: string;
  readonly navigate: (path: string, replace?: boolean) => void;
  readonly children: ReactNode;
}) {
  const client = useManagementClient();
  const [project, setProject] = useState<Project | null>(null);
  const [environments, setEnvironments] = useState<Environment[]>([]);
  const [destinations, setDestinations] = useState<WorkspaceDestination[]>([]);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  useEffect(() => {
    let active = true;
    void Promise.all([
      client.getProject(projectId),
      client.listEnvironments(projectId),
      client.getWorkspaceNavigation(projectId, environmentId),
    ]).then(
      ([nextProject, nextEnvironments, nextDestinations]) => {
        if (!active) return;
        setProject(nextProject);
        setEnvironments(nextEnvironments);
        setDestinations(nextDestinations);
        setFailure(null);
      },
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
    };
  }, [client, environmentId, projectId]);
  const currentEnvironment = environments.find((environment) => environment.id === environmentId);
  return (
    <div className="developer-workspace-shell">
      <aside className="developer-sidebar" aria-label="Environment navigation">
        <div className="context-switcher">
          <p className="eyebrow">Database workspace</p>
          <button
            type="button"
            className="context-home"
            onClick={() => navigate(`/projects/${projectId}`)}
          >
            {project?.name ?? projectId}
          </button>
          <label>
            Environment
            <select
              aria-label="Switch environment"
              value={environmentId}
              onChange={(event) =>
                navigate(
                  `/projects/${projectId}/environments/${event.currentTarget.value}/overview`,
                )
              }
            >
              {environments.map((environment) => (
                <option key={environment.id} value={environment.id}>
                  {environment.name} · {environment.state}
                </option>
              ))}
            </select>
          </label>
        </div>
        <nav aria-label="Environment destinations">
          <ul>
            {[
              ...destinations.filter((destination) => destination.permitted),
              ...consoleDestinations(projectId, environmentId),
            ].map((destination) => (
              <li key={destination.id}>
                <a
                  className={section === destination.id ? "active" : ""}
                  aria-current={section === destination.id ? "page" : undefined}
                  href={destination.path}
                  onClick={(event) => {
                    event.preventDefault();
                    navigate(destination.path);
                  }}
                >
                  {destination.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>
      </aside>
      <div className="developer-workspace-content">
        <nav className="breadcrumbs" aria-label="Breadcrumb">
          <a
            href="/"
            onClick={(event) => {
              event.preventDefault();
              navigate("/");
            }}
          >
            Teams
          </a>
          <span aria-hidden="true">/</span>
          <a
            href={`/projects/${projectId}`}
            onClick={(event) => {
              event.preventDefault();
              navigate(`/projects/${projectId}`);
            }}
          >
            {project?.name ?? projectId}
          </a>
          <span aria-hidden="true">/</span>
          <strong>{currentEnvironment?.name ?? environmentId}</strong>
        </nav>
        <ApiFailureNotice failure={failure} />
        {children}
      </div>
    </div>
  );
}

function WorkspaceOverview({
  projectId,
  environmentId,
  navigate,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly navigate: (path: string) => void;
}) {
  const client = useManagementClient();
  const [summary, setSummary] = useState<WorkspaceSummary | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setSummary(await client.getWorkspaceSummary(projectId, environmentId));
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);
  return (
    <section>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment overview</p>
          <h1>Database health and activity</h1>
          <p>Each card loads independently; unavailable providers do not erase healthy sections.</p>
        </div>
        <button type="button" className="secondary" onClick={() => void reload()}>
          Refresh
        </button>
      </div>
      <ApiFailureNotice failure={failure} />
      {summary === null ? (
        <p>Loading workspace summaries…</p>
      ) : (
        <div className="summary-grid">
          {Object.entries(summary.sections).map(([id, item]) => (
            <article className="summary-card" key={id}>
              <div className="section-heading">
                <h2>{humanize(id)}</h2>
                <StatusBadge state={item.status} />
              </div>
              <SummaryPayload value={item.payload} />
              <small>
                Observed {formatTime(item.observedAtUnixSeconds)} · fresh until{" "}
                {formatTime(item.freshUntilUnixSeconds)}
                {item.retainedSinceUnixSeconds !== null &&
                item.retainedSinceUnixSeconds !== undefined
                  ? ` · retained since ${formatTime(item.retainedSinceUnixSeconds)}`
                  : ""}
              </small>
              {item.remediationCode !== null && item.remediationCode !== undefined ? (
                <p className="notice warning">{humanize(item.remediationCode)}</p>
              ) : null}
            </article>
          ))}
        </div>
      )}
      <div className="quick-actions">
        <button
          type="button"
          onClick={() => navigate(`/projects/${projectId}/environments/${environmentId}/data`)}
        >
          Explore data
        </button>
        <button
          type="button"
          className="secondary"
          onClick={() => navigate(`/projects/${projectId}/environments/${environmentId}/connect`)}
        >
          Connect RxDB
        </button>
        <button
          type="button"
          className="secondary"
          onClick={() => navigate(`/projects/${projectId}/environments/${environmentId}/sync`)}
        >
          Inspect sync
        </button>
      </div>
    </section>
  );
}

function ConnectPage({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [metadata, setMetadata] = useState<ConnectMetadata | null>(null);
  const [collectionId, setCollectionId] = useState("");
  const [check, setCheck] = useState<ConnectionCheck | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  useEffect(() => {
    let active = true;
    void client.getConnectMetadata(projectId, environmentId).then(
      (value) => {
        if (!active) return;
        setMetadata(value);
        setCollectionId(value.collections[0]?.collectionId ?? "");
      },
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
    };
  }, [client, environmentId, projectId]);
  const snippet = useMemo(
    () => (metadata === null || collectionId === "" ? "" : rxdbSnippet(metadata, collectionId)),
    [collectionId, metadata],
  );
  const runCheck = async () => {
    try {
      const collection = metadata?.collections.find((item) => item.collectionId === collectionId);
      setCheck(
        await client.checkConnection(projectId, environmentId, {
          ...(metadata === null ? {} : { publicKeyId: metadata.publicKeyId }),
          ...(collection === undefined
            ? {}
            : {
                collectionId: collection.collectionId,
                schemaVersion: collection.activeSchemaVersion,
              }),
          rxdbVersion: "17.0.0",
        }),
      );
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  return (
    <section>
      <div className="section-heading">
        <div>
          <p className="eyebrow">API & Connect</p>
          <h1>Connect an RxDB application</h1>
          <p>
            Only the public project key belongs in browser or mobile code. Never ship a service
            credential.
          </p>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {metadata === null ? (
        <p>Loading public connection metadata…</p>
      ) : (
        <>
          <div className="definition-grid">
            <div>
              <dt>Public endpoint</dt>
              <dd>
                <code>{metadata.publicEndpoint}</code>
              </dd>
            </div>
            <div>
              <dt>Public key ID</dt>
              <dd>
                <code>{metadata.publicKeyId}</code>
              </dd>
            </div>
            <div>
              <dt>Supported client</dt>
              <dd>
                <code>{metadata.rxdbClientRange}</code>
              </dd>
            </div>
            <div>
              <dt>Template</dt>
              <dd>v{metadata.templateVersion}</dd>
            </div>
          </div>
          {metadata.publicKey === "" ? (
            <p className="notice warning">
              No recoverable public key is available. Issue or rotate a public project credential on
              the Credentials page, then paste the one-time value into your application secret
              store.
            </p>
          ) : (
            <label>
              Public project key
              <input readOnly value={metadata.publicKey} />
            </label>
          )}
          <label>
            Collection
            <select
              value={collectionId}
              onChange={(event) => setCollectionId(event.currentTarget.value)}
            >
              {metadata.collections.map((collection) => (
                <option key={collection.collectionId} value={collection.collectionId}>
                  {collection.collectionId} · schema v{collection.activeSchemaVersion}
                </option>
              ))}
            </select>
          </label>
          <div className="code-block">
            <button
              type="button"
              className="secondary copy-button"
              onClick={() => void navigator.clipboard.writeText(snippet)}
            >
              Copy
            </button>
            <pre>{snippet}</pre>
          </div>
          <section className="panel">
            <div className="section-heading">
              <div>
                <h2>Connection check</h2>
                <p>
                  Checks DNS, TLS, public routing, readiness, key metadata, schema compatibility,
                  and replication routes without reading documents or creating a user session.
                </p>
              </div>
              <button type="button" onClick={() => void runCheck()}>
                Run check
              </button>
            </div>
            {check !== null ? (
              <ol className="check-list">
                {check.steps.map((step) => (
                  <li key={step.id}>
                    <StatusBadge state={step.state} />
                    <span>
                      <strong>{humanize(step.id)}</strong>
                      {step.remediationCode !== null && step.remediationCode !== undefined ? (
                        <small>
                          {humanize(step.remediationCode)} ·{" "}
                          {step.retryable ? "retryable" : "configuration change required"}
                        </small>
                      ) : null}
                    </span>
                  </li>
                ))}
              </ol>
            ) : null}
          </section>
        </>
      )}
    </section>
  );
}

function SyncDashboard({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [collections, setCollections] = useState<Collection[]>([]);
  const [summary, setSummary] = useState<SyncSummary | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const query = useCallback(
    async (event?: FormEvent<HTMLFormElement>) => {
      event?.preventDefault();
      try {
        const data = event === undefined ? new FormData() : new FormData(event.currentTarget);
        const hours = Number(data.get("hours") ?? 1);
        const until = Math.floor(Date.now() / 1000);
        const collectionId = data.get("collectionId");
        setSummary(
          await client.getSyncSummary(projectId, environmentId, {
            ...(typeof collectionId === "string" && collectionId !== "" ? { collectionId } : {}),
            from: until - Math.max(1, Math.min(24, hours)) * 3600,
            until,
          }),
        );
        setFailure(null);
      } catch (error) {
        setFailure(toConsoleApiFailure(error));
      }
    },
    [client, environmentId, projectId],
  );
  useEffect(() => {
    void client
      .listCollections(projectId, environmentId)
      .then(setCollections, (error: unknown) => setFailure(toConsoleApiFailure(error)));
    void query();
  }, [client, environmentId, projectId, query]);
  const metrics =
    summary === null
      ? []
      : ([
          ["Pulls", summary.pullCount],
          ["Pushes", summary.pushCount],
          ["Live streams", summary.liveStreams],
          ["Lag p95 ms", summary.lagP95Milliseconds],
          ["Conflicts", summary.conflicts],
          ["Policy denials", summary.policyDenials],
          ["Throttled", summary.throttled],
          ["Checkpoint expired", summary.checkpointExpired],
          ["Stream gaps", summary.streamGaps],
          ["Resyncs", summary.resyncs],
          ["Schema mismatch", summary.schemaMismatches],
        ] as const);
  return (
    <section>
      <p className="eyebrow">RxDB sync</p>
      <h1>Replication diagnostics</h1>
      <ApiFailureNotice failure={failure} />
      <form className="filter-grid" onSubmit={(event) => void query(event)}>
        <label>
          Collection
          <select name="collectionId">
            <option value="">All collections</option>
            {collections.map((collection) => (
              <option key={collection.id} value={collection.id}>
                {collection.id}
              </option>
            ))}
          </select>
        </label>
        <label>
          Time window
          <select name="hours" defaultValue="1">
            <option value="1">Last hour</option>
            <option value="6">Last 6 hours</option>
            <option value="24">Last 24 hours</option>
          </select>
        </label>
        <button type="submit">Apply</button>
      </form>
      {summary === null ? (
        <p>Loading sync summary…</p>
      ) : (
        <>
          <div className="metric-grid">
            {metrics.map(([label, value]) => (
              <article key={label}>
                <strong>{value ?? 0}</strong>
                <span>{label}</span>
              </article>
            ))}
          </div>
          <section className="panel">
            <h2>Client compatibility classes</h2>
            {Object.keys(summary.clientVersionClasses ?? {}).length === 0 ? (
              <p>No bounded client-version observations in this window.</p>
            ) : (
              <ul>
                {Object.entries(summary.clientVersionClasses ?? {}).map(([label, count]) => (
                  <li key={label}>
                    {humanize(label)}: {count}
                  </li>
                ))}
              </ul>
            )}
            <p>
              <small>
                Observed {formatTime(summary.observedAtUnixSeconds)} · retained since{" "}
                {formatTime(summary.retainedSinceUnixSeconds)}. Counts never contain raw user,
                device, session, IP, token, or document identifiers.
              </small>
            </p>
          </section>
          <RemediationHelp summary={summary} />
        </>
      )}
    </section>
  );
}

function RemediationHelp({ summary }: { readonly summary: SyncSummary }) {
  const issues = [
    { count: summary.throttled, text: "Throttling is retryable after the server-provided delay." },
    {
      count: summary.checkpointExpired,
      text: "Expired checkpoints require a confirmed full resync.",
    },
    { count: summary.streamGaps, text: "Stream gaps require a confirmed full resync." },
    {
      count: summary.schemaMismatches,
      text: "Schema mismatches require an RxDB migration before retrying.",
    },
    {
      count: summary.policyDenials,
      text: "Policy denials are non-retryable until policy or user authorization changes.",
    },
  ].filter((item) => (item.count ?? 0) > 0);
  return issues.length === 0 ? null : (
    <aside className="notice warning">
      <strong>Recommended action</strong>
      <ul>
        {issues.map((issue) => (
          <li key={issue.text}>{issue.text}</li>
        ))}
      </ul>
    </aside>
  );
}

function BackupAndRestore({
  projectId,
  environmentId,
  restoreEnabled,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly restoreEnabled: boolean;
}) {
  const client = useManagementClient();
  const [backups, setBackups] = useState<DeveloperBackup[]>([]);
  const [restores, setRestores] = useState<DeveloperRestore[]>([]);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      const [nextBackups, nextRestores] = await Promise.all([
        client.listDeveloperBackups(projectId, environmentId),
        client.listDeveloperRestoreRequests(projectId),
      ]);
      setBackups(nextBackups);
      setRestores(
        nextRestores.filter(
          (restore) => restore.target.environmentId !== environmentId || restore.backupId !== "",
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);
  const requestRestore = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      const stepUp = await client.verifyCurrentDeveloperPassword(requiredText(data, "password"));
      await client.requestDeveloperRestore(
        projectId,
        {
          environmentId,
          backupId: requiredText(data, "backupId"),
          targetEnvironmentName: requiredText(data, "targetEnvironmentName"),
          reason: requiredText(data, "reason"),
          stepUpToken: stepUp.token,
        },
        crypto.randomUUID(),
      );
      form.reset();
      await reload();
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  return (
    <section>
      <p className="eyebrow">Backup and recovery</p>
      <h1>Verified recovery points</h1>
      <p>
        Developer recovery always creates a new isolated environment. It cannot overwrite or promote
        an environment.
      </p>
      <ApiFailureNotice failure={failure} />
      {backups.length === 0 ? (
        <p className="notice">No tenant-verified recovery point is available.</p>
      ) : (
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Recovery point</th>
                <th>Verification</th>
                <th>Retention</th>
                <th>Restore drill / objective</th>
              </tr>
            </thead>
            <tbody>
              {backups.map((backup) => (
                <tr key={backup.backupId}>
                  <td>
                    <code>{backup.backupId}</code>
                    <br />
                    {formatTime(backup.recoveryPointUnixSeconds)}
                  </td>
                  <td>{formatTime(backup.verifiedAtUnixSeconds)}</td>
                  <td>{formatTime(backup.retainedUntilUnixSeconds)}</td>
                  <td>
                    {backup.lastRestoreDrillUnixSeconds === null ||
                    backup.lastRestoreDrillUnixSeconds === undefined
                      ? "No recorded drill"
                      : formatTime(backup.lastRestoreDrillUnixSeconds)}
                    <br />
                    {backup.recoveryObjectiveStatus}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {restoreEnabled ? (
        <section className="panel">
          <h2>Request isolated recovery environment</h2>
          <p className="notice warning">
            The restored environment stays inaccessible until tenant isolation, storage, services,
            and recovery validation all pass. Overwrite and promotion are prohibited.
          </p>
          <form onSubmit={(event) => void requestRestore(event)}>
            <label>
              Verified backup
              <select name="backupId" required>
                {backups.map((backup) => (
                  <option key={backup.backupId} value={backup.backupId}>
                    {backup.backupId} · {formatTime(backup.recoveryPointUnixSeconds)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              New environment name
              <input name="targetEnvironmentName" required maxLength={64} />
            </label>
            <label>
              Reason
              <textarea name="reason" required maxLength={500} />
            </label>
            <label>
              Confirm your developer password
              <input name="password" type="password" required autoComplete="current-password" />
            </label>
            <button type="submit" disabled={backups.length === 0}>
              Request recovery
            </button>
          </form>
        </section>
      ) : (
        <DisabledWorkspaceFeature name="Isolated recovery requests" />
      )}
      <section className="panel">
        <h2>Recovery requests</h2>
        {restores.length === 0 ? (
          <p>No recovery requests.</p>
        ) : (
          <div className="resource-list">
            {restores.map((restore) => (
              <div className="resource-row static" key={restore.requestId}>
                <span>
                  <strong>{restore.target.environmentId}</strong>
                  <small>
                    {restore.state} · updated {formatTime(restore.updatedAtUnixSeconds)}
                  </small>
                </span>
                <StatusBadge state={restore.accessible ? "ready" : restore.state} />
              </div>
            ))}
          </div>
        )}
      </section>
    </section>
  );
}

function EnvironmentSettings({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [environment, setEnvironment] = useState<Environment | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  useEffect(() => {
    let active = true;
    void client.getEnvironment(projectId, environmentId).then(
      (value) => active && setEnvironment(value),
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
    };
  }, [client, environmentId, projectId]);
  return (
    <section>
      <p className="eyebrow">Settings</p>
      <h1>Environment settings</h1>
      <ApiFailureNotice failure={failure} />
      {environment === null ? (
        <p>Loading…</p>
      ) : (
        <dl className="definition-grid">
          <div>
            <dt>Name</dt>
            <dd>{environment.name}</dd>
          </div>
          <div>
            <dt>ID</dt>
            <dd>
              <code>{environment.id}</code>
            </dd>
          </div>
          <div>
            <dt>Lifecycle</dt>
            <dd>
              <StatusBadge state={environment.state} />
            </dd>
          </div>
          <div>
            <dt>Updated</dt>
            <dd>{new Date(environment.updatedAt).toLocaleString()}</dd>
          </div>
        </dl>
      )}
      <p className="notice">
        Credentials, schema, function, and destructive lifecycle controls remain on their existing
        dedicated pages while the workspace rollout is reversible.
      </p>
      <AllowedOriginsSection projectId={projectId} environmentId={environmentId} />
    </section>
  );
}

function WorkspaceLinks({
  title,
  description,
  links,
  navigate,
}: {
  readonly title: string;
  readonly description: string;
  readonly links: readonly { readonly label: string; readonly path: string }[];
  readonly navigate: (path: string) => void;
}) {
  return (
    <section>
      <p className="eyebrow">Workspace</p>
      <h1>{title}</h1>
      <p>{description}</p>
      <div className="resource-list">
        {links.map((link) => (
          <button
            type="button"
            className="resource-row"
            key={link.path}
            onClick={() => navigate(link.path)}
          >
            {link.label}
            <span aria-hidden="true">→</span>
          </button>
        ))}
      </div>
    </section>
  );
}
function DisabledWorkspaceFeature({ name }: { readonly name: string }) {
  return (
    <section className="panel">
      <p className="eyebrow">Staged rollout</p>
      <h1>{name}</h1>
      <p>
        This capability is disabled by its independent deployment gate. Existing project tools
        remain available.
      </p>
    </section>
  );
}
function StatusBadge({ state }: { readonly state: string }) {
  return (
    <span
      className={`status-badge ${state === "current" || state === "passed" || state === "ready" || state === "active" ? "success" : state === "unavailable" || state === "failed" ? "error" : "warning"}`}
    >
      {humanize(state)}
    </span>
  );
}
function SummaryPayload({ value }: { readonly value: unknown }) {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return <p>{value === undefined ? "No summary payload" : String(value)}</p>;
  return (
    <dl>
      {Object.entries(value)
        .slice(0, 12)
        .map(([key, item]) => (
          <div key={key}>
            <dt>{humanize(key)}</dt>
            <dd>{typeof item === "object" ? JSON.stringify(item) : String(item)}</dd>
          </div>
        ))}
    </dl>
  );
}
function humanize(value: string): string {
  const spaced = value.replace(/([a-z])([A-Z])/gu, "$1 $2").replaceAll("_", " ");
  return `${spaced.charAt(0).toUpperCase()}${spaced.slice(1)}`;
}
function formatTime(value: number): string {
  return new Date(value * 1000).toLocaleString();
}
function requiredText(data: FormData, name: string): string {
  const value = data.get(name);
  if (typeof value !== "string" || value.trim() === "")
    throw new Error(`${humanize(name)} is required.`);
  return value.trim();
}
function consoleFailure(error: unknown): ConsoleApiFailure {
  return error instanceof Error && !("requestId" in error)
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}
function rxdbSnippet(metadata: ConnectMetadata, collectionId: string): string {
  const collection = metadata.collections.find((item) => item.collectionId === collectionId);
  return createMakoRxdbConnectTemplateV1({
    endpoint: metadata.publicEndpoint,
    projectId: metadata.tenant.projectId,
    environmentId: metadata.tenant.environmentId,
    collectionId,
    schemaVersion: collection?.activeSchemaVersion ?? 1,
    publicProjectKey: metadata.publicKey || "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY",
  });
}
