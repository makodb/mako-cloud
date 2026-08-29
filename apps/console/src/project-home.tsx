// Project home: the project-level shell with overview, usage, activity, and
// settings. It is a read-only aggregation of endpoints the console already
// calls — project, environments, owner, and for the selected environment the
// workspace summary, connect metadata, usage, health, and audit events. Every
// summary loads on its own and states when it was observed, so one failing
// source marks only its own panel.
import {
  type FormEvent,
  type MouseEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import type {
  ConnectMetadata,
  Environment,
  MakoManagementClient,
  ObservabilityPage,
  Project,
  Team,
  WorkspaceSummary,
} from "@mako-cloud/management-sdk";
import { createMakoRxdbConnectTemplateV1 } from "@mako-cloud/rxdb";

import { ActivityScreen } from "./activity.js";
import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { CustomDomainsScreen } from "./custom-domains.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction } from "./safety.js";
import { UsageScreen } from "./usage.js";

export type ProjectSection = "overview" | "usage" | "activity" | "settings" | "domains";

type ProjectAction = "suspend" | "restore" | "delete";
type Navigate = (path: string, replace?: boolean) => void;

const PROJECT_DESTINATIONS: readonly { readonly id: ProjectSection; readonly label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "usage", label: "Usage" },
  { id: "activity", label: "Activity" },
  { id: "domains", label: "Domains" },
  { id: "settings", label: "Settings" },
];

// The transfer target that names the caller's personal space; the API
// resolves it when no team is named, so it needs no identifier here.
const PERSONAL_TARGET = "personal";

const SECTION_EYEBROW: Record<ProjectSection, string> = {
  overview: "Project overview",
  usage: "Project usage",
  activity: "Project activity",
  domains: "Custom domains",
  settings: "Project settings",
};

export function ProjectHome({
  projectId,
  section,
  navigate,
}: {
  readonly projectId: string;
  readonly section: ProjectSection;
  readonly navigate: Navigate;
}) {
  const client = useManagementClient();
  const [project, setProject] = useState<Project | null>(null);
  const [environments, setEnvironments] = useState<Environment[] | null>(null);
  const [owner, setOwner] = useState<Team | "unavailable" | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [selectedEnvironmentId, setSelectedEnvironmentId] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const [nextProject, nextEnvironments] = await Promise.all([
        client.getProject(projectId),
        client.listEnvironments(projectId),
      ]);
      setProject(nextProject);
      setEnvironments(nextEnvironments);
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  // The owner is resolved once per project; a missing owner name never blocks
  // the project itself, the identifier stands in for it.
  const teamId = project?.teamId ?? null;
  useEffect(() => {
    if (teamId === null) return;
    let active = true;
    void client.getTeam(teamId).then(
      (team) => active && setOwner(team),
      () => active && setOwner("unavailable"),
    );
    return () => {
      active = false;
    };
  }, [client, teamId]);

  const provisioning =
    project?.state === "provisioning" ||
    (environments?.some((environment) => environment.state === "provisioning") ?? false);
  useEffect(() => {
    if (!provisioning) return;
    const timer = window.setInterval(() => void reload(), 5_000);
    return () => window.clearInterval(timer);
  }, [provisioning, reload]);

  const selectedEnvironment = useMemo(
    () =>
      environments === null
        ? null
        : (environments.find((environment) => environment.id === selectedEnvironmentId) ??
          environments.find((environment) => environment.state === "active") ??
          environments[0] ??
          null),
    [environments, selectedEnvironmentId],
  );

  const projectAction = async (action: ProjectAction) => {
    if (
      action !== "restore" &&
      !confirmDestructiveAction({
        action: action === "delete" ? "Request deletion for" : "Suspend",
        target: `project ${project?.name ?? projectId}`,
        consequence:
          action === "delete"
            ? "Data-plane access will be revoked and final destruction will follow the displayed grace period."
            : "Application traffic for this project will be interrupted until it is restored.",
      })
    ) {
      return;
    }
    try {
      const updated =
        action === "suspend"
          ? await client.suspendProject(projectId, idempotencyKey())
          : action === "restore"
            ? await client.restoreProject(projectId, idempotencyKey())
            : await client.requestProjectDeletion(projectId, deletionConfirmation(project?.name));
      setProject(updated);
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };

  const team = owner === "unavailable" ? null : owner;
  const ownerLabel =
    team === null
      ? (project?.teamId ?? "Owner")
      : team.kind === "personal"
        ? "Your projects"
        : team.name;
  const ownerPath = project === null ? "/" : `/teams/${project.teamId}`;

  let content: ReactNode;
  if (project === null || environments === null) {
    content = <p>Loading project…</p>;
  } else if (section === "usage") {
    content = <UsageScreen projectId={projectId} />;
  } else if (section === "activity") {
    content = <ActivityScreen projectId={projectId} />;
  } else if (section === "domains") {
    content = <CustomDomainsScreen projectId={projectId} environments={environments} />;
  } else if (section === "settings") {
    content = (
      <ProjectSettings
        project={project}
        team={team}
        ownerUnavailable={owner === "unavailable"}
        onAction={(action) => void projectAction(action)}
        onChanged={reload}
      />
    );
  } else {
    content = (
      <ProjectOverview
        projectId={projectId}
        project={project}
        environments={environments}
        selectedEnvironment={selectedEnvironment}
        onSelectEnvironment={setSelectedEnvironmentId}
        onProjectAction={(action) => void projectAction(action)}
        onChanged={reload}
        navigate={navigate}
      />
    );
  }

  return (
    <div className="developer-workspace-shell project-home">
      <aside className="developer-sidebar" aria-label="Project navigation">
        <div className="context-switcher">
          <p className="eyebrow">Project</p>
          <ConsoleLink className="project-home-owner" path={ownerPath} navigate={navigate}>
            {ownerLabel}
          </ConsoleLink>
          <button
            type="button"
            className="context-home"
            onClick={() => navigate(`/projects/${projectId}`)}
          >
            {project?.name ?? projectId}
          </button>
          <div className="project-home-environments">
            <p id="project-environments-label">Environments</p>
            {environments === null ? (
              <small>Loading…</small>
            ) : environments.length === 0 ? (
              <small>No environments yet</small>
            ) : (
              <ul aria-labelledby="project-environments-label">
                {environments.map((environment) => (
                  <li key={environment.id}>
                    <ConsoleLink
                      path={`/projects/${projectId}/environments/${environment.id}/overview`}
                      navigate={navigate}
                    >
                      <span>{environment.name}</span>
                      <LifecycleBadge state={environment.state} />
                    </ConsoleLink>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
        <nav aria-label="Project destinations">
          <ul>
            {PROJECT_DESTINATIONS.map((destination) => (
              <li key={destination.id}>
                <ConsoleLink
                  className={section === destination.id ? "active" : undefined}
                  current={section === destination.id}
                  path={
                    destination.id === "overview"
                      ? `/projects/${projectId}`
                      : `/projects/${projectId}/${destination.id}`
                  }
                  navigate={navigate}
                >
                  {destination.label}
                </ConsoleLink>
              </li>
            ))}
          </ul>
        </nav>
      </aside>
      <div className="developer-workspace-content">
        <nav className="breadcrumbs" aria-label="Breadcrumb">
          <ConsoleLink path="/" navigate={navigate}>
            Home
          </ConsoleLink>
          <span aria-hidden="true">/</span>
          <ConsoleLink path={ownerPath} navigate={navigate}>
            {ownerLabel}
          </ConsoleLink>
          <span aria-hidden="true">/</span>
          <strong>{project?.name ?? projectId}</strong>
        </nav>
        <div className="section-heading project-home-heading">
          <div>
            <p className="eyebrow">{SECTION_EYEBROW[section]}</p>
            <h1 id="project-title">{project?.name ?? "Loading…"}</h1>
            {project === null ? null : (
              <p className="project-home-meta">
                <LifecycleBadge state={project.state} />
                <span>Region {project.region}</span>
                <code>{project.id}</code>
              </p>
            )}
          </div>
        </div>
        <ApiFailureNotice failure={failure} />
        {content}
      </div>
    </div>
  );
}

function ProjectOverview({
  projectId,
  project,
  environments,
  selectedEnvironment,
  onSelectEnvironment,
  onProjectAction,
  onChanged,
  navigate,
}: {
  readonly projectId: string;
  readonly project: Project;
  readonly environments: readonly Environment[];
  readonly selectedEnvironment: Environment | null;
  readonly onSelectEnvironment: (environmentId: string) => void;
  readonly onProjectAction: (action: ProjectAction) => void;
  readonly onChanged: () => Promise<void>;
  readonly navigate: Navigate;
}) {
  const [refreshKey, setRefreshKey] = useState(0);
  return (
    <>
      <EnvironmentsPanel
        projectId={projectId}
        environments={environments}
        selectedEnvironmentId={selectedEnvironment?.id ?? null}
        onSelect={onSelectEnvironment}
        onChanged={onChanged}
        navigate={navigate}
      />
      {selectedEnvironment === null ? null : (
        <section aria-labelledby="selected-environment-title">
          <div className="section-heading project-home-selected">
            <div>
              <p className="eyebrow">Selected environment</p>
              <h2 id="selected-environment-title">{selectedEnvironment.name}</h2>
              <p>
                Each summary loads on its own and names when it was observed. An unavailable summary
                never hides a healthy one.
              </p>
            </div>
            <button
              type="button"
              className="secondary"
              onClick={() => setRefreshKey((value) => value + 1)}
            >
              Refresh summaries
            </button>
          </div>
          <div className="project-home-grid" key={`${selectedEnvironment.id}:${refreshKey}`}>
            <ConnectPanel
              projectId={projectId}
              environment={selectedEnvironment}
              navigate={navigate}
            />
            <UsagePanel
              projectId={projectId}
              environment={selectedEnvironment}
              navigate={navigate}
            />
            <HealthPanel
              projectId={projectId}
              environment={selectedEnvironment}
              navigate={navigate}
            />
            <ActivityPanel
              projectId={projectId}
              environment={selectedEnvironment}
              navigate={navigate}
            />
          </div>
        </section>
      )}
      <ProjectLifecyclePanel project={project} onAction={onProjectAction} />
    </>
  );
}

function EnvironmentsPanel({
  projectId,
  environments,
  selectedEnvironmentId,
  onSelect,
  onChanged,
  navigate,
}: {
  readonly projectId: string;
  readonly environments: readonly Environment[];
  readonly selectedEnvironmentId: string | null;
  readonly onSelect: (environmentId: string) => void;
  readonly onChanged: () => Promise<void>;
  readonly navigate: Navigate;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const createEnvironment = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    try {
      await client.createEnvironment(
        projectId,
        String(new FormData(form).get("name") ?? "").trim(),
        idempotencyKey(),
      );
      form.reset();
      setFailure(null);
      await onChanged();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  return (
    <section className="panel full-span project-home-panel" aria-labelledby="environments-title">
      <div className="section-heading">
        <div>
          <h2 id="environments-title">Environments</h2>
          <p>
            Each environment reports its own readiness. Select one to see its keys, usage, health,
            and activity.
          </p>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {environments.length === 0 ? (
        <p>This project has no environments yet. Create one to start connecting clients.</p>
      ) : (
        <div className="table-scroll">
          <table className="project-home-table">
            <thead>
              <tr>
                <th scope="col">
                  <span className="visually-hidden">Selected</span>
                </th>
                <th scope="col">Environment</th>
                <th scope="col">State</th>
                <th scope="col">Readiness</th>
                <th scope="col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {environments.map((environment) => (
                <EnvironmentTableRow
                  key={environment.id}
                  environment={environment}
                  selected={environment.id === selectedEnvironmentId}
                  onSelect={() => onSelect(environment.id)}
                  onChanged={onChanged}
                  navigate={navigate}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      <details>
        <summary>Create environment</summary>
        <form onSubmit={(event) => void createEnvironment(event)}>
          <label>
            Environment name
            <input name="name" required maxLength={100} />
          </label>
          <button type="submit">Create environment</button>
        </form>
      </details>
    </section>
  );
}

function EnvironmentTableRow({
  environment,
  selected,
  onSelect,
  onChanged,
  navigate,
}: {
  readonly environment: Environment;
  readonly selected: boolean;
  readonly onSelect: () => void;
  readonly onChanged: () => Promise<void>;
  readonly navigate: Navigate;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const act = async (action: ProjectAction) => {
    if (
      action !== "restore" &&
      !confirmDestructiveAction({
        action: action === "delete" ? "Request deletion for" : "Suspend",
        target: `environment ${environment.name}`,
        consequence:
          action === "delete"
            ? "Environment access will be revoked and final destruction will follow its grace period."
            : "Application traffic for this environment will be interrupted until it is restored.",
      })
    ) {
      return;
    }
    try {
      if (action === "suspend") {
        await client.suspendEnvironment(environment.projectId, environment.id, idempotencyKey());
      } else if (action === "restore") {
        await client.restoreEnvironment(environment.projectId, environment.id, idempotencyKey());
      } else {
        await client.requestEnvironmentDeletion(
          environment.projectId,
          environment.id,
          deletionConfirmation(environment.name),
        );
      }
      setFailure(null);
      await onChanged();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  return (
    <tr aria-selected={selected}>
      <td>
        <input
          type="radio"
          name="selected-environment"
          aria-label={`Select ${environment.name}`}
          checked={selected}
          onChange={onSelect}
        />
      </td>
      <td>
        <strong>{environment.name}</strong>
        <br />
        <code>{environment.id}</code>
        {environment.deletionDeadline === undefined ? null : (
          <small className="project-home-observed">
            Restorable until {new Date(environment.deletionDeadline).toLocaleString()}
          </small>
        )}
      </td>
      <td>
        <LifecycleBadge state={environment.state} />
      </td>
      <td>
        <EnvironmentReadiness projectId={environment.projectId} environmentId={environment.id} />
      </td>
      <td>
        <div className="button-row">
          <ConsoleLink
            className="project-home-open"
            path={`/projects/${environment.projectId}/environments/${environment.id}/overview`}
            navigate={navigate}
          >
            Open
          </ConsoleLink>
          <button
            type="button"
            className="secondary"
            disabled={environment.state !== "active"}
            onClick={() => void act("suspend")}
          >
            Suspend
          </button>
          <button
            type="button"
            className="secondary"
            disabled={!(["suspended", "deletion_grace"] as string[]).includes(environment.state)}
            onClick={() => void act("restore")}
          >
            Restore
          </button>
          <button
            type="button"
            className="danger-link"
            disabled={!(["active", "suspended", "failed"] as string[]).includes(environment.state)}
            onClick={() => void act("delete")}
          >
            Delete
          </button>
        </div>
        <ApiFailureNotice failure={failure} />
      </td>
    </tr>
  );
}

/// Readiness comes from the environment's workspace summary. The control
/// plane serves it as the `lifecycle` section (`ready`, project and
/// environment lifecycle); an older `readiness` section with a `state` is
/// read the same way. Each row loads on its own, so one environment whose
/// summary fails is marked unavailable without touching its siblings.
function EnvironmentReadiness({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const load = useCallback(
    (client: MakoManagementClient) => client.getWorkspaceSummary(projectId, environmentId),
    [environmentId, projectId],
  );
  const summary = useSummary(load);
  if (summary.status === "loading") {
    return <span className="project-home-observed">Loading readiness…</span>;
  }
  if (summary.status === "unavailable") {
    return (
      <div className="project-home-readiness">
        <StatusBadge state="unavailable" />
        <small className="project-home-observed">{summary.failure.message}</small>
      </div>
    );
  }
  const section = summary.value.sections.lifecycle ?? summary.value.sections.readiness;
  if (section === undefined) {
    return (
      <div className="project-home-readiness">
        <StatusBadge state="unavailable" />
        <small className="project-home-observed">
          No readiness section · observed {summary.observedAt.toLocaleString()}
        </small>
      </div>
    );
  }
  const detail = readinessDetail(section.payload);
  const counts = summaryCounts(summary.value);
  return (
    <div className="project-home-readiness">
      <span>
        <StatusBadge state={section.status} />
        {detail === null ? null : <span> {detail}</span>}
      </span>
      {counts === null ? null : <small>{counts}</small>}
      <small className="project-home-observed">
        Observed {formatUnixSeconds(section.observedAtUnixSeconds)}
      </small>
      {section.remediationCode === null || section.remediationCode === undefined ? null : (
        <small className="project-home-observed">{humanize(section.remediationCode)}</small>
      )}
    </div>
  );
}

function ConnectPanel({
  projectId,
  environment,
  navigate,
}: {
  readonly projectId: string;
  readonly environment: Environment;
  readonly navigate: Navigate;
}) {
  const load = useCallback(
    (client: MakoManagementClient) => client.getConnectMetadata(projectId, environment.id),
    [environment.id, projectId],
  );
  const metadata = useSummary(load);
  const base = `/projects/${projectId}/environments/${environment.id}`;
  return (
    <SummaryPanel
      id="connect"
      title="Keys and API URL"
      description="Only the public project key belongs in browser or mobile code. Never ship a service credential."
      state={metadata}
      observedAt={(_, observedAt) => observedAt}
      actions={
        <>
          <ConsoleLink path={`${base}/connect`} navigate={navigate}>
            Open Connect
          </ConsoleLink>
          <ConsoleLink path={`${base}/credentials`} navigate={navigate}>
            Manage credentials
          </ConsoleLink>
        </>
      }
    >
      {(value) => <ConnectDetails metadata={value} />}
    </SummaryPanel>
  );
}

function ConnectDetails({ metadata }: { readonly metadata: ConnectMetadata }) {
  const collection = metadata.collections[0];
  const snippet = createMakoRxdbConnectTemplateV1({
    endpoint: metadata.publicEndpoint,
    projectId: metadata.tenant.projectId,
    environmentId: metadata.tenant.environmentId,
    collectionId: collection?.collectionId ?? "todos",
    schemaVersion: collection?.activeSchemaVersion ?? 1,
    publicProjectKey: metadata.publicKey || "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY",
  });
  return (
    <>
      <dl className="definition-grid">
        <div>
          <dt>API URL</dt>
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
      </dl>
      {metadata.publicKey === "" ? (
        <p className="notice warning">
          No recoverable public key is available. Issue or rotate a public project credential on the
          Credentials page, then paste the one-time value into your application secret store.
        </p>
      ) : (
        <label className="project-home-key">
          Public project key
          <input readOnly value={metadata.publicKey} />
        </label>
      )}
      <div>
        <p>
          <strong>Quickstart</strong>
          {collection === undefined ? (
            <small className="project-home-observed">
              No collection exists yet; the snippet uses a placeholder collection.
            </small>
          ) : (
            <small className="project-home-observed">
              Collection {collection.collectionId} · schema v{collection.activeSchemaVersion}
            </small>
          )}
        </p>
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
      </div>
    </>
  );
}

function UsagePanel({
  projectId,
  environment,
  navigate,
}: {
  readonly projectId: string;
  readonly environment: Environment;
  readonly navigate: Navigate;
}) {
  const load = useCallback(
    (client: MakoManagementClient) =>
      client.queryProjectUsage(projectId, environment.id, { limit: 50 }),
    [environment.id, projectId],
  );
  const page = useSummary(load);
  return (
    <SummaryPanel
      id="usage"
      title="Usage"
      description="Metered quantities in the retention window, summed per resource."
      state={page}
      observedAt={(value) => new Date(value.retention.observedAt)}
      actions={
        <ConsoleLink path={`/projects/${projectId}/usage`} navigate={navigate}>
          Open usage and quota
        </ConsoleLink>
      }
    >
      {(value) => {
        const totals = usageTotals(value);
        return totals.length === 0 ? (
          <p>No usage has been recorded in the retention window.</p>
        ) : (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th scope="col">Resource</th>
                  <th scope="col">Quantity</th>
                </tr>
              </thead>
              <tbody>
                {totals.map((total) => (
                  <tr key={total.resource}>
                    <td>{total.resource.replaceAll("_", " ")}</td>
                    <td>
                      {formatQuantity(total.resource, total.quantity)}
                      {total.resource.includes("bytes") ? "" : ` ${total.unit}`}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      }}
    </SummaryPanel>
  );
}

function HealthPanel({
  projectId,
  environment,
  navigate,
}: {
  readonly projectId: string;
  readonly environment: Environment;
  readonly navigate: Navigate;
}) {
  const load = useCallback(
    (client: MakoManagementClient) =>
      client.queryProjectHealth(projectId, environment.id, { limit: 20 }),
    [environment.id, projectId],
  );
  const page = useSummary(load);
  return (
    <SummaryPanel
      id="health"
      title="Data-plane health"
      description="Retained health observations for the services behind this environment."
      state={page}
      observedAt={(value) => new Date(value.retention.observedAt)}
      actions={
        <ConsoleLink
          path={`/projects/${projectId}/environments/${environment.id}/observability`}
          navigate={navigate}
        >
          Open observability
        </ConsoleLink>
      }
    >
      {(value) => {
        const health = value.items.flatMap((record) =>
          record.payload.kind === "health"
            ? [{ timestamp: record.timestamp, payload: record.payload }]
            : [],
        );
        return health.length === 0 ? (
          <p>No retained health observations.</p>
        ) : (
          <ul className="signal-list">
            {health.map((record) => (
              <li key={`${record.timestamp}-${record.payload.service}-${record.payload.region}`}>
                <strong>{record.payload.service}</strong> in {record.payload.region}:{" "}
                {record.payload.status}
                {record.payload.diagnostic === null ? null : (
                  <small className="project-home-observed">{record.payload.diagnostic}</small>
                )}
              </li>
            ))}
          </ul>
        );
      }}
    </SummaryPanel>
  );
}

function ActivityPanel({
  projectId,
  environment,
  navigate,
}: {
  readonly projectId: string;
  readonly environment: Environment;
  readonly navigate: Navigate;
}) {
  const load = useCallback(
    (client: MakoManagementClient) =>
      client.queryAuditEvents(projectId, environment.id, { limit: 10 }),
    [environment.id, projectId],
  );
  const page = useSummary(load);
  return (
    <SummaryPanel
      id="activity"
      title="Recent activity"
      description="The newest audited actions in this environment."
      state={page}
      observedAt={(value) => new Date(value.retention.observedAt)}
      actions={
        <ConsoleLink path={`/projects/${projectId}/activity`} navigate={navigate}>
          Open activity
        </ConsoleLink>
      }
    >
      {(value) => {
        const events = auditEvents(value);
        return events.length === 0 ? (
          <p>No audited actions in the retention window.</p>
        ) : (
          <ol className="project-home-events">
            {events.map((event) => (
              <li key={`${event.timestamp}-${event.requestId}`}>
                <time dateTime={event.timestamp}>{new Date(event.timestamp).toLocaleString()}</time>
                <span>
                  <strong>{event.actorId}</strong> {event.action} <code>{event.target}</code>
                </span>
                <small className="project-home-observed">
                  {event.outcome}
                  {event.details === null ? "" : ` · ${event.details}`}
                </small>
              </li>
            ))}
          </ol>
        );
      }}
    </SummaryPanel>
  );
}

function ProjectLifecyclePanel({
  project,
  onAction,
}: {
  readonly project: Project;
  readonly onAction: (action: ProjectAction) => void;
}) {
  return (
    <section
      className="panel full-span project-home-panel lifecycle-panel"
      aria-labelledby="lifecycle-title"
    >
      <h2 id="lifecycle-title">Provisioning and lifecycle</h2>
      <p>
        Current state: <strong>{project.state.replaceAll("_", " ")}</strong>
      </p>
      {project.failureDiagnostic === undefined ? null : (
        <p role="alert">{project.failureDiagnostic}</p>
      )}
      {project.deletionDeadline === undefined ? null : (
        <p>Restorable until {new Date(project.deletionDeadline).toLocaleString()}.</p>
      )}
      <div className="button-row">
        <button
          type="button"
          onClick={() => onAction("suspend")}
          disabled={project.state !== "active"}
        >
          Suspend
        </button>
        <button
          type="button"
          className="secondary"
          onClick={() => onAction("restore")}
          disabled={!(["suspended", "deletion_grace"] as string[]).includes(project.state)}
        >
          Restore
        </button>
        <button
          type="button"
          className="danger"
          onClick={() => onAction("delete")}
          disabled={!(["active", "suspended", "failed"] as string[]).includes(project.state)}
        >
          Request deletion
        </button>
      </div>
    </section>
  );
}

function ProjectSettings({
  project,
  team,
  ownerUnavailable,
  onAction,
  onChanged,
}: {
  readonly project: Project;
  readonly team: Team | null;
  readonly ownerUnavailable: boolean;
  readonly onAction: (action: ProjectAction) => void;
  readonly onChanged: () => Promise<void>;
}) {
  return (
    <>
      <section className="panel full-span project-home-panel" aria-labelledby="identifiers-title">
        <h2 id="identifiers-title">Identifiers and ownership</h2>
        <dl className="definition-grid">
          <div>
            <dt>Project ID</dt>
            <dd>
              <code>{project.id}</code>
            </dd>
          </div>
          <div>
            <dt>Owner</dt>
            <dd>
              {team === null
                ? ownerUnavailable
                  ? "Owner unavailable"
                  : "Loading…"
                : team.kind === "personal"
                  ? "Your personal space"
                  : team.name}
            </dd>
          </div>
          <div>
            <dt>Owner kind</dt>
            <dd>{team === null ? "—" : team.kind === "personal" ? "Personal space" : "Team"}</dd>
          </div>
          <div>
            <dt>Owner ID</dt>
            <dd>
              <code>{project.teamId}</code>
            </dd>
          </div>
          <div>
            <dt>Region</dt>
            <dd>{project.region}</dd>
          </div>
          <div>
            <dt>Lifecycle</dt>
            <dd>
              <LifecycleBadge state={project.state} />
            </dd>
          </div>
          <div>
            <dt>Created</dt>
            <dd>{new Date(project.createdAt).toLocaleString()}</dd>
          </div>
          <div>
            <dt>Updated</dt>
            <dd>{new Date(project.updatedAt).toLocaleString()}</dd>
          </div>
        </dl>
      </section>
      <RenameProjectPanel project={project} onChanged={onChanged} />
      <TransferProjectPanel project={project} owner={team} onChanged={onChanged} />
      <section className="panel full-span project-home-panel" aria-labelledby="deletion-title">
        <h2 id="deletion-title">Deletion</h2>
        <p>
          Requesting deletion revokes data-plane access immediately. Final destruction follows a
          grace period during which the project can be restored.
        </p>
        {project.failureDiagnostic === undefined ? null : (
          <p role="alert">{project.failureDiagnostic}</p>
        )}
        {project.deletionDeadline === undefined ? null : (
          <p>Restorable until {new Date(project.deletionDeadline).toLocaleString()}.</p>
        )}
        <div className="button-row">
          <button
            type="button"
            className="danger"
            onClick={() => onAction("delete")}
            disabled={!(["active", "suspended", "failed"] as string[]).includes(project.state)}
          >
            Request deletion
          </button>
          <button
            type="button"
            className="secondary"
            onClick={() => onAction("restore")}
            disabled={project.state !== "deletion_grace"}
          >
            Restore
          </button>
        </div>
      </section>
    </>
  );
}

/// Renaming changes the display name only; the identifier, keys, policies,
/// and data stay. The draft follows the project's current name until the
/// developer edits it, so a rename that arrives through a reload is shown
/// rather than overwritten.
function RenameProjectPanel({
  project,
  onChanged,
}: {
  readonly project: Project;
  readonly onChanged: () => Promise<void>;
}) {
  const client = useManagementClient();
  const [draft, setDraft] = useState<string | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const name = draft ?? project.name;
  const nextName = name.trim();
  const unchanged = nextName === "" || nextName === project.name;
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (unchanged || pending) return;
    if (
      !confirmDestructiveAction({
        action: "Rename",
        target: `project ${project.name}`,
        consequence: `The project will be called "${nextName}" everywhere it is listed. Its identifier, keys, policies, and data are unchanged.`,
      })
    ) {
      return;
    }
    setPending(true);
    setFailure(null);
    setStatus(null);
    try {
      const renamed = await client.updateProject(project.id, nextName);
      await onChanged();
      setDraft(null);
      setStatus(`Renamed to ${renamed.name}. The change is audited.`);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <section className="panel full-span project-home-panel" aria-labelledby="rename-title">
      <h2 id="rename-title">Project name</h2>
      <p>
        The name appears everywhere the project is listed. Its identifier, keys, policies, and data
        never change with it. Renaming requires confirmation and is audited.
      </p>
      <form className="inline-form project-home-form" onSubmit={(event) => void submit(event)}>
        <label>
          Project name
          <input
            name="name"
            value={name}
            maxLength={200}
            onChange={(event) => setDraft(event.currentTarget.value)}
          />
        </label>
        <button type="submit" disabled={unchanged || pending}>
          {pending ? "Renaming…" : "Rename"}
        </button>
      </form>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
    </section>
  );
}

/// Transfer offers every team the developer belongs to and the personal
/// space, less the current owner. The wire type carries no role, so the
/// console does not guess who administers what: the API refuses a target the
/// developer does not administer, and that refusal is shown as is.
function TransferProjectPanel({
  project,
  owner,
  onChanged,
}: {
  readonly project: Project;
  readonly owner: Team | null;
  readonly onChanged: () => Promise<void>;
}) {
  const client = useManagementClient();
  const load = useCallback((management: MakoManagementClient) => management.listTeams(), []);
  const teams = useSummary(load);
  const [selection, setSelection] = useState("");
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const listed = teams.status === "ready" ? teams.value : [];
  const ownerIsPersonal =
    owner?.kind === "personal" ||
    listed.some((team) => team.kind === "personal" && team.id === project.teamId);
  const teamTargets = listed.filter((team) => team.kind === "team" && team.id !== project.teamId);
  const target: Team | typeof PERSONAL_TARGET | null =
    selection === PERSONAL_TARGET
      ? ownerIsPersonal
        ? null
        : PERSONAL_TARGET
      : (teamTargets.find((team) => team.id === selection) ?? null);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (target === null || pending) return;
    const targetName = target === PERSONAL_TARGET ? "your personal space" : target.name;
    if (
      !confirmDestructiveAction({
        action: "Transfer",
        target: `project ${project.name} to ${
          target === PERSONAL_TARGET ? targetName : `team ${targetName}`
        }`,
        consequence:
          "Policies, users, data, and keys stay with the project. Usage limits follow the new owner's plan, and access follows the new owner's membership.",
      })
    ) {
      return;
    }
    setPending(true);
    setFailure(null);
    setStatus(null);
    try {
      await client.transferProject(
        project.id,
        target === PERSONAL_TARGET ? undefined : target.id,
        `transfer:${project.id}`,
      );
      await onChanged();
      setSelection("");
      setStatus(`Transferred to ${targetName}; the move is audited under both owners.`);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setPending(false);
    }
  };

  let form: ReactNode;
  if (teams.status === "loading") {
    form = <p aria-busy="true">Loading the teams you belong to…</p>;
  } else if (teams.status === "unavailable") {
    form = (
      <>
        <p>The teams you belong to could not be listed, so no transfer can be offered right now.</p>
        <ApiFailureNotice failure={teams.failure} />
      </>
    );
  } else if (ownerIsPersonal && teamTargets.length === 0) {
    form = (
      <p>
        This project is in your personal space and you belong to no team, so there is no owner to
        transfer it to.
      </p>
    );
  } else {
    form = (
      <form className="inline-form project-home-form" onSubmit={(event) => void submit(event)}>
        <label>
          Transfer to
          <select value={selection} onChange={(event) => setSelection(event.currentTarget.value)}>
            <option value="">Choose the new owner</option>
            {ownerIsPersonal ? null : (
              <option value={PERSONAL_TARGET}>Your projects (personal space)</option>
            )}
            {teamTargets.map((team) => (
              <option key={team.id} value={team.id}>
                {team.name}
                {team.state === "active" ? "" : ` (${humanize(team.state).toLowerCase()})`}
              </option>
            ))}
          </select>
        </label>
        <button type="submit" disabled={target === null || pending}>
          {pending ? "Transferring…" : "Transfer"}
        </button>
      </form>
    );
  }
  return (
    <section className="panel full-span project-home-panel" aria-labelledby="transfer-title">
      <h2 id="transfer-title">Transfer ownership</h2>
      <p>
        Move this project to your personal space or to a team you administer. Its identifier,
        environments, policies, users, data, and keys stay as they are; usage limits follow the new
        owner's plan. A transfer requires confirmation and is audited under both owners.
      </p>
      {form}
      <ApiFailureNotice failure={failure} />
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
    </section>
  );
}

type SummaryState<T> =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly value: T; readonly observedAt: Date }
  | { readonly status: "unavailable"; readonly failure: ConsoleApiFailure };

/// One independent fetch per summary. Callers memoize `load`; a change of
/// environment remounts the panel through its key, so the state starts over.
function useSummary<T>(load: (client: MakoManagementClient) => Promise<T>): SummaryState<T> {
  const client = useManagementClient();
  const [state, setState] = useState<SummaryState<T>>({ status: "loading" });
  useEffect(() => {
    let active = true;
    void load(client).then(
      (value) => active && setState({ status: "ready", value, observedAt: new Date() }),
      (error: unknown) =>
        active && setState({ status: "unavailable", failure: toConsoleApiFailure(error) }),
    );
    return () => {
      active = false;
    };
  }, [client, load]);
  return state;
}

function SummaryPanel<T>({
  id,
  title,
  description,
  state,
  observedAt,
  actions,
  children,
}: {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly state: SummaryState<T>;
  readonly observedAt: (value: T, fetchedAt: Date) => Date;
  readonly actions?: ReactNode;
  readonly children: (value: T) => ReactNode;
}) {
  const badge =
    state.status === "ready" ? "current" : state.status === "loading" ? "loading" : "unavailable";
  return (
    <section
      className="panel project-home-panel"
      aria-labelledby={`${id}-summary-title`}
      data-status={state.status}
    >
      <div className="section-heading">
        <div>
          <h2 id={`${id}-summary-title`}>{title}</h2>
          <p>{description}</p>
        </div>
        <StatusBadge state={badge} />
      </div>
      {state.status === "loading" ? (
        <p aria-busy="true">Loading…</p>
      ) : state.status === "unavailable" ? (
        <>
          <p>This summary is unavailable; the other summaries are unaffected.</p>
          <ApiFailureNotice failure={state.failure} />
        </>
      ) : (
        <>
          {children(state.value)}
          <small className="project-home-observed">
            Observed {observedAt(state.value, state.observedAt).toLocaleString()}
          </small>
        </>
      )}
      {actions === undefined ? null : <div className="project-home-actions">{actions}</div>}
    </section>
  );
}

function ConsoleLink({
  path,
  navigate,
  className,
  current = false,
  children,
}: {
  readonly path: string;
  readonly navigate: Navigate;
  readonly className?: string | undefined;
  readonly current?: boolean;
  readonly children: ReactNode;
}) {
  return (
    <a
      href={path}
      className={className}
      aria-current={current ? "page" : undefined}
      onClick={(event: MouseEvent<HTMLAnchorElement>) => {
        if (
          event.metaKey ||
          event.ctrlKey ||
          event.shiftKey ||
          event.altKey ||
          event.button !== 0
        ) {
          return;
        }
        event.preventDefault();
        navigate(path);
      }}
    >
      {children}
    </a>
  );
}

function StatusBadge({ state }: { readonly state: string }) {
  const tone =
    state === "current" || state === "ready" || state === "active"
      ? "success"
      : state === "unavailable" || state === "failed"
        ? "error"
        : "warning";
  return <span className={`status-badge ${tone}`}>{humanize(state)}</span>;
}

function readinessDetail(payload: unknown): string | null {
  if (typeof payload !== "object" || payload === null || Array.isArray(payload)) return null;
  const record = payload as Record<string, unknown>;
  if (typeof record.ready === "boolean") {
    return record.ready ? "Ready" : "Not ready";
  }
  if (typeof record.state === "string") {
    return humanize(record.state);
  }
  return null;
}

function summaryCounts(summary: WorkspaceSummary): string | null {
  const parts: string[] = [];
  for (const [id, label] of [
    ["collections", "collections"],
    ["functions", "functions"],
  ] as const) {
    const payload = summary.sections[id]?.payload;
    if (typeof payload === "object" && payload !== null && !Array.isArray(payload)) {
      const count = (payload as Record<string, unknown>).count;
      if (typeof count === "number") parts.push(`${count} ${label}`);
    }
  }
  return parts.length === 0 ? null : parts.join(" · ");
}

function usageTotals(
  page: ObservabilityPage,
): readonly { readonly resource: string; readonly quantity: number; readonly unit: string }[] {
  const totals = new Map<string, { quantity: number; unit: string }>();
  for (const record of page.items) {
    if (record.payload.kind !== "usage") continue;
    const current = totals.get(record.payload.resource);
    if (current === undefined) {
      totals.set(record.payload.resource, {
        quantity: record.payload.quantity,
        unit: record.payload.unit,
      });
    } else {
      current.quantity += record.payload.quantity;
    }
  }
  return [...totals.entries()]
    .map(([resource, total]) => ({ resource, ...total }))
    .sort((left, right) => left.resource.localeCompare(right.resource));
}

function auditEvents(page: ObservabilityPage): readonly {
  readonly timestamp: string;
  readonly actorId: string;
  readonly action: string;
  readonly target: string;
  readonly outcome: string;
  readonly requestId: string;
  readonly details: string | null;
}[] {
  return page.items
    .flatMap((record) =>
      record.payload.kind === "audit" ? [{ timestamp: record.timestamp, ...record.payload }] : [],
    )
    .sort((left, right) => right.timestamp.localeCompare(left.timestamp));
}

function formatQuantity(resource: string, value: number): string {
  if (!resource.includes("bytes")) return value.toLocaleString("en-US");
  const units = ["B", "KiB", "MiB", "GiB", "TiB"] as const;
  let scaled = value;
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  return index === 0 ? `${value} B` : `${scaled.toFixed(1)} ${units[index] ?? "B"}`;
}

function humanize(value: string): string {
  const spaced = value.replace(/([a-z])([A-Z])/gu, "$1 $2").replaceAll("_", " ");
  return `${spaced.charAt(0).toUpperCase()}${spaced.slice(1)}`;
}

function formatUnixSeconds(value: number): string {
  return new Date(value * 1000).toLocaleString();
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}

function deletionConfirmation(name: string | undefined): string {
  return `delete:${name ?? "resource"}`;
}
