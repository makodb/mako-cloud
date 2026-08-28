import { type FormEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import type {
  CreateOperatorRecoveryJobRequest,
  OperatorActivityPage,
  OperatorAlertPage,
  OperatorIncident,
  OperatorIncidentPage,
  OperatorInventoryKind,
  OperatorPermission,
  OperatorReadSection,
  OperatorSecurityInventory,
  OperatorTenant360,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useOperatorAuth } from "./operator-auth.js";
import { useOperatorClient } from "./operator-management.js";
import {
  AbuseResponsePanel,
  QuotaOverridePanel,
  RepairPanel,
  SupportSessionPanel,
} from "./operator.js";
import { OperatorWaitListPanel } from "./operator-waitlist.js";

export type OperatorSection =
  | "overview"
  | "tenants"
  | "tenant"
  | "operations"
  | "incidents"
  | "backups"
  | "sync"
  | "fleet"
  | "storage"
  | "security"
  | "activity"
  | "waitlist";

interface OperatorWorkspaceProps {
  readonly section: OperatorSection;
  readonly projectId: string | undefined;
  readonly navigate: (path: string) => void;
  readonly onExit: () => void;
}

interface NavigationItem {
  readonly section: Exclude<OperatorSection, "tenant">;
  readonly label: string;
  readonly permission?: string;
}

const NAVIGATION: readonly NavigationItem[] = [
  { section: "overview", label: "Overview", permission: "overview_read" },
  { section: "tenants", label: "Tenants", permission: "tenant_read" },
  { section: "operations", label: "Operations", permission: "operations_read" },
  { section: "incidents", label: "Incidents", permission: "incident_read" },
  { section: "backups", label: "Backups & recovery", permission: "backup_read" },
  { section: "sync", label: "RxDB sync", permission: "fleet_read" },
  { section: "fleet", label: "Fleet", permission: "fleet_read" },
  { section: "storage", label: "Storage", permission: "fleet_read" },
  { section: "security", label: "Security", permission: "security_read" },
  { section: "activity", label: "Activity", permission: "activity_read" },
  { section: "waitlist", label: "Developer wait list", permission: "waitlist_review" },
];

export function OperatorWorkspaceScreen({
  section,
  projectId,
  navigate,
  onExit,
}: OperatorWorkspaceProps) {
  const { state, signOut } = useOperatorAuth();
  if (state.status !== "authenticated") return null;
  const { session } = state;
  const permissions = new Set(session.permissions);
  const visibleNavigation = NAVIGATION.filter(
    (item) => item.permission === undefined || allowsRead(permissions, item.permission),
  );
  const defaultSection = visibleNavigation[0]?.section ?? "overview";
  const effectiveSection =
    window.location.pathname.replace(/\/$/u, "") === "/operator" &&
    section === "overview" &&
    !allowsRead(permissions, "overview_read")
      ? defaultSection
      : section;
  const selected = effectiveSection === "tenant" ? "tenants" : effectiveSection;
  return (
    <div className="operator-workspace-shell">
      <a className="skip-link" href="#operator-main-content">
        Skip to operator workspace
      </a>
      <header className="operator-topbar operator-control-topbar">
        <div>
          <p className="eyebrow">Audited platform administration</p>
          <strong>Mako Cloud Control Center</strong>
        </div>
        <div className="account operator-session-summary">
          <span>{session.profile.email}</span>
          <span title={session.expiresAt}>Session expires {formatRelative(session.expiresAt)}</span>
          <SupportModeStatus />
          <button type="button" className="secondary" onClick={onExit}>
            Developer site
          </button>
          <button type="button" onClick={() => void signOut()}>
            Sign out
          </button>
        </div>
      </header>
      <aside className="operator-sidebar" aria-label="Operator workspace">
        <nav>
          {visibleNavigation.map((item) => (
            <button
              key={item.section}
              type="button"
              className={selected === item.section ? "operator-nav-active" : "operator-nav-item"}
              aria-current={selected === item.section ? "page" : undefined}
              onClick={() => navigate(`/operator/${item.section}`)}
            >
              {item.label}
            </button>
          ))}
        </nav>
        <p className="operator-boundary-note">
          Routine pages expose metadata and aggregate signals only. Document content requires a
          separately verified, scoped support session.
        </p>
      </aside>
      <main className="operator-main" id="operator-main-content" tabIndex={-1}>
        <OperatorRoute
          section={effectiveSection}
          projectId={projectId}
          navigate={navigate}
          permissions={permissions}
        />
      </main>
    </div>
  );
}

function SupportModeStatus() {
  const client = useOperatorClient();
  const loader = useCallback(() => client.listCurrentOperatorSupportSessions(25), [client]);
  const resource = useResource(loader);
  if (resource.status === "loading") {
    return <span className="status-pill unknown">Support mode: checking</span>;
  }
  if (resource.status === "error") {
    return <span className="status-pill unavailable">Support mode: unknown</span>;
  }
  if (resource.value.length === 0) {
    return <span className="status-pill unknown">Support mode: inactive</span>;
  }
  const scopes = resource.value.map((grant) =>
    grant.environmentId === null ? grant.projectId : `${grant.projectId}/${grant.environmentId}`,
  );
  return (
    <span className="status-pill stale" title={scopes.join(", ")}>
      Support mode: active ({resource.value.length})
    </span>
  );
}

function OperatorRoute({
  section,
  projectId,
  navigate,
  permissions,
}: {
  readonly section: OperatorSection;
  readonly projectId: string | undefined;
  readonly navigate: (path: string) => void;
  readonly permissions: ReadonlySet<string>;
}) {
  if (section === "overview") return <OverviewPage />;
  if (section === "tenants") return <TenantDirectoryPage navigate={navigate} />;
  if (section === "tenant" && projectId !== undefined) {
    return <Tenant360Page projectId={projectId} navigate={navigate} />;
  }
  if (section === "incidents") return <IncidentsPage permissions={permissions} />;
  if (section === "activity") return <ActivityPage permissions={permissions} />;
  if (section === "security") return <SecurityPage permissions={permissions} />;
  if (section === "waitlist") {
    return (
      <WorkspacePage title="Developer registration wait list" eyebrow="Identity admission">
        <OperatorWaitListPanel />
      </WorkspacePage>
    );
  }
  const inventory = inventoryForSection(section);
  if (inventory !== null) {
    return (
      <InventoryPage
        kind={inventory.kind}
        title={inventory.title}
        description={inventory.description}
        showRecovery={section === "backups" && permissions.has("recovery_manage")}
        showOperations={section === "operations"}
        permissions={permissions}
      />
    );
  }
  return (
    <WorkspacePage title="Operator page unavailable" eyebrow="Safe direct-route failure">
      <p role="alert">This route is not available to the current operator entitlement.</p>
    </WorkspacePage>
  );
}

function OverviewPage() {
  const client = useOperatorClient();
  const loader = useCallback(() => client.getOperatorOverview(), [client]);
  const resource = useResource(loader);
  return (
    <WorkspacePage
      title="Platform overview"
      eyebrow="Exceptions first"
      actions={<ReloadButton reload={resource.reload} pending={resource.status === "loading"} />}
    >
      <ResourceFailure resource={resource} />
      {resource.status !== "ready" ? (
        <LoadingState label="Loading bounded platform summaries…" />
      ) : (
        <>
          <div className="operator-stat-grid">
            <StatCard label="Active tenants" value={resource.value.activeTenants} tone="current" />
            <StatCard
              label="Need attention"
              value={resource.value.attentionTenants}
              tone={resource.value.attentionTenants > 0 ? "unavailable" : "current"}
            />
            <StatCard
              label="Provider state"
              value={resource.value.partial ? "Partial" : "Complete"}
              tone={resource.value.partial ? "stale" : "current"}
            />
          </div>
          <SectionGrid sections={resource.value.sections} />
        </>
      )}
    </WorkspacePage>
  );
}

function TenantDirectoryPage({ navigate }: { readonly navigate: (path: string) => void }) {
  const client = useOperatorClient();
  const [query, setQuery] = useState("");
  const [submittedQuery, setSubmittedQuery] = useState("");
  const [cursor, setCursor] = useState<string | undefined>();
  const [cursorHistory, setCursorHistory] = useState<(string | undefined)[]>([]);
  const loader = useCallback(
    () =>
      client.listOperatorTenants({
        ...(submittedQuery === "" ? {} : { query: submittedQuery }),
        ...(cursor === undefined ? {} : { cursor }),
        limit: 25,
      }),
    [client, submittedQuery, cursor],
  );
  const resource = useResource(loader);
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setSubmittedQuery(query.trim());
    setCursor(undefined);
    setCursorHistory([]);
  };
  return (
    <WorkspacePage
      title="Tenant directory"
      eyebrow="Bounded global inventory"
      actions={<ReloadButton reload={resource.reload} pending={resource.status === "loading"} />}
    >
      <form className="operator-filter-bar" onSubmit={submit}>
        <label>
          Team, project, environment, region, developer email, or identifier
          <input
            type="search"
            value={query}
            maxLength={200}
            onChange={(event) => setQuery(event.currentTarget.value)}
          />
        </label>
        <button type="submit">Search</button>
      </form>
      <ResourceFailure resource={resource} />
      {resource.status !== "ready" ? (
        <LoadingState label="Loading a bounded tenant page…" />
      ) : resource.value.items.length === 0 ? (
        <EmptyState title="No tenants match this bounded page" />
      ) : (
        <>
          <div className="operator-table-scroll">
            <table>
              <thead>
                <tr>
                  <th scope="col">Project</th>
                  <th scope="col">Team</th>
                  <th scope="col">Lifecycle</th>
                  <th scope="col">Region</th>
                  <th scope="col">Environments</th>
                  <th scope="col">Health</th>
                  <th scope="col">Action</th>
                </tr>
              </thead>
              <tbody>
                {resource.value.items.map((tenant) => (
                  <tr key={tenant.projectId}>
                    <td>
                      <strong>{tenant.projectName}</strong>
                      <br />
                      <code>{tenant.projectId}</code>
                    </td>
                    <td>{tenant.teamName ?? tenant.teamId}</td>
                    <td>{humanize(tenant.lifecycle)}</td>
                    <td>{tenant.region}</td>
                    <td>{tenant.environmentCount}</td>
                    <td>
                      <FreshnessBadge value={tenant.health} />
                    </td>
                    <td>
                      <button
                        type="button"
                        className="secondary"
                        onClick={() => navigate(`/operator/tenants/${tenant.projectId}`)}
                      >
                        Open Tenant 360
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <CursorControls
            canPrevious={cursorHistory.length > 0}
            canNext={resource.value.nextCursor !== null}
            onPrevious={() => {
              const previous = [...cursorHistory];
              setCursor(previous.pop());
              setCursorHistory(previous);
            }}
            onNext={() => {
              if (resource.value.nextCursor === null) return;
              setCursorHistory((history) => [...history, cursor]);
              setCursor(resource.value.nextCursor);
            }}
          />
        </>
      )}
    </WorkspacePage>
  );
}

function Tenant360Page({
  projectId,
  navigate,
}: {
  readonly projectId: string;
  readonly navigate: (path: string) => void;
}) {
  const client = useOperatorClient();
  const loader = useCallback(() => client.getOperatorTenant360(projectId), [client, projectId]);
  const resource = useResource(loader);
  return (
    <WorkspacePage
      title="Tenant 360"
      eyebrow={projectId}
      actions={
        <div className="button-row">
          <button type="button" className="secondary" onClick={() => navigate("/operator/tenants")}>
            Back to tenants
          </button>
          <ReloadButton reload={resource.reload} pending={resource.status === "loading"} />
        </div>
      }
    >
      <ResourceFailure resource={resource} />
      {resource.status !== "ready" ? (
        <LoadingState label="Loading independently sourced tenant sections…" />
      ) : (
        <Tenant360Content value={resource.value} />
      )}
    </WorkspacePage>
  );
}

function Tenant360Content({ value }: { readonly value: OperatorTenant360 }) {
  return (
    <>
      <section className="panel operator-tenant-identity">
        <div>
          <p className="eyebrow">{value.project.teamName ?? value.project.teamId}</p>
          <h2>{value.project.projectName}</h2>
          <code>{value.project.projectId}</code>
        </div>
        <dl className="operator-definition-grid">
          <div>
            <dt>Lifecycle</dt>
            <dd>{humanize(value.project.lifecycle)}</dd>
          </div>
          <div>
            <dt>Region</dt>
            <dd>{value.project.region}</dd>
          </div>
          <div>
            <dt>Plan</dt>
            <dd>{value.project.plan}</dd>
          </div>
          <div>
            <dt>Health</dt>
            <dd>
              <FreshnessBadge value={value.project.health} />
            </dd>
          </div>
        </dl>
      </section>
      {value.partial ? (
        <p className="notice warning" role="status">
          One or more providers are unavailable. Successful sections remain visible and unavailable
          sections do not imply healthy state.
        </p>
      ) : null}
      <section className="panel">
        <h2>Topology</h2>
        {value.environments.length === 0 ? (
          <p>No environments exist.</p>
        ) : (
          <ul className="operator-topology-list">
            {value.environments.map((environment) => (
              <li key={environment.id}>
                <strong>{environment.name}</strong> <code>{environment.id}</code>{" "}
                <span>{humanize(environment.state)}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
      <SectionGrid sections={Object.values(value.sections)} />
    </>
  );
}

function InventoryPage({
  kind,
  title,
  description,
  showRecovery,
  showOperations,
  permissions,
}: {
  readonly kind: OperatorInventoryKind;
  readonly title: string;
  readonly description: string;
  readonly showRecovery: boolean;
  readonly showOperations: boolean;
  readonly permissions: ReadonlySet<string>;
}) {
  const client = useOperatorClient();
  const [projectId, setProjectId] = useState("");
  const [scope, setScope] = useState<string | undefined>();
  const loader = useCallback(
    () => client.getOperatorInventory(kind, scope === undefined ? {} : { projectId: scope }),
    [client, kind, scope],
  );
  const resource = useResource(loader);
  return (
    <WorkspacePage
      title={title}
      eyebrow="Bounded aggregate view"
      actions={<ReloadButton reload={resource.reload} pending={resource.status === "loading"} />}
    >
      <p>{description}</p>
      <form
        className="operator-filter-bar"
        onSubmit={(event) => {
          event.preventDefault();
          setScope(projectId.trim() === "" ? undefined : projectId.trim());
        }}
      >
        <label>
          Optional project scope
          <input
            value={projectId}
            pattern="prj_[A-Za-z0-9_-]{8,64}"
            placeholder="prj_…"
            onChange={(event) => setProjectId(event.currentTarget.value)}
          />
        </label>
        <button type="submit">Apply scope</button>
      </form>
      <ResourceFailure resource={resource} />
      {resource.status !== "ready" ? (
        <LoadingState label={`Loading ${title.toLocaleLowerCase()}…`} />
      ) : (
        <SectionGrid sections={resource.value.items} />
      )}
      {showOperations ? (
        <ContextualOperationsPanel projectId={scope} permissions={permissions} />
      ) : null}
      {showRecovery ? (
        <>
          <RecoveryJobsPanel />
          <RecoveryRequestPanel />
        </>
      ) : null}
    </WorkspacePage>
  );
}

function ContextualOperationsPanel({
  projectId,
  permissions,
}: {
  readonly projectId: string | undefined;
  readonly permissions: ReadonlySet<string>;
}) {
  const client = useOperatorClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [workflow, setWorkflow] = useState<Awaited<
    ReturnType<typeof client.repairOperatorProvisioning>
  > | null>(null);
  const [quota, setQuota] = useState<Awaited<
    ReturnType<typeof client.createOperatorQuotaOverride>
  > | null>(null);
  const [abuse, setAbuse] = useState<Awaited<
    ReturnType<typeof client.createOperatorAbuseResponse>
  > | null>(null);
  const [support, setSupport] = useState<Awaited<
    ReturnType<typeof client.createSupportSession>
  > | null>(null);
  const loader = useCallback(async () => {
    if (projectId === undefined) return null;
    const [project, workflows, quotas, abuseResponses, supportSessions] = await Promise.all([
      client.getOperatorProject(projectId),
      client.listOperatorProvisioningWorkflows({ projectId, limit: 25 }),
      client.listOperatorQuotaOverrides(projectId, 25),
      client.listOperatorAbuseResponses(projectId, 25),
      client.listOperatorSupportSessions(projectId, 25),
    ]);
    return { project, workflows, quotas, abuseResponses, supportSessions };
  }, [client, projectId]);
  const resource = useResource(loader);
  if (projectId === undefined)
    return (
      <section className="panel">
        <h2>Contextual workflows</h2>
        <p>
          Apply an exact project scope above before loading or changing provisioning, quota, abuse,
          or support state.
        </p>
      </section>
    );
  return (
    <section className="full-span" aria-label="Contextual operational workflows">
      <ApiFailureNotice failure={failure} />
      {resource.status !== "ready" || resource.value === null ? (
        <LoadingState label="Loading exact-project workflow history…" />
      ) : (
        <>
          <section className="panel">
            <h2>Selected tenant workflow inventory</h2>
            <p>
              <code>{projectId}</code> · {resource.value.workflows.length} provisioning ·{" "}
              {resource.value.quotas.length} quota · {resource.value.abuseResponses.length} abuse ·{" "}
              {resource.value.supportSessions.length} support records.
            </p>
          </section>
          <div className="split-grid stacked-section">
            {permissions.has("provisioning_repair") ? (
              <RepairPanel
                workflow={workflow}
                onValidationError={(message) => setFailure({ message, requestId: null })}
                onSubmit={async (workflowId, action, reason) => {
                  try {
                    setWorkflow(
                      await client.repairOperatorProvisioning(projectId, workflowId, {
                        action,
                        reason,
                        operationKey: `operator-repair-${crypto.randomUUID()}`,
                      }),
                    );
                    setFailure(null);
                    resource.reload();
                  } catch (error) {
                    setFailure(toConsoleApiFailure(error));
                  }
                }}
              />
            ) : null}
            {permissions.has("quota_override") ? (
              <QuotaOverridePanel
                result={quota}
                onValidationError={(message) => setFailure({ message, requestId: null })}
                onSubmit={async (input) => {
                  try {
                    setQuota(await client.createOperatorQuotaOverride(projectId, input));
                    setFailure(null);
                    resource.reload();
                  } catch (error) {
                    setFailure(toConsoleApiFailure(error));
                  }
                }}
              />
            ) : null}
            {permissions.has("abuse_response") ? (
              <AbuseResponsePanel
                environments={resource.value.project.environments.map(
                  (environment) => environment.id,
                )}
                result={abuse}
                onValidationError={(message) => setFailure({ message, requestId: null })}
                onSubmit={async (input) => {
                  try {
                    setAbuse(await client.createOperatorAbuseResponse(projectId, input));
                    setFailure(null);
                    resource.reload();
                  } catch (error) {
                    setFailure(toConsoleApiFailure(error));
                  }
                }}
              />
            ) : null}
            {permissions.has("support_access") ? (
              <SupportSessionPanel
                environments={resource.value.project.environments.map(
                  (environment) => environment.id,
                )}
                session={support}
                onValidationError={(message) => setFailure({ message, requestId: null })}
                onCreate={async (input) => {
                  try {
                    setSupport(await client.createSupportSession(projectId, input));
                    setFailure(null);
                    resource.reload();
                  } catch (error) {
                    setFailure(toConsoleApiFailure(error));
                  }
                }}
                onRevoke={async (sessionId, reason) => {
                  try {
                    setSupport(await client.revokeSupportSession(projectId, sessionId, reason));
                    setFailure(null);
                    resource.reload();
                  } catch (error) {
                    setFailure(toConsoleApiFailure(error));
                  }
                }}
              />
            ) : null}
          </div>
        </>
      )}
    </section>
  );
}

function RecoveryJobsPanel() {
  const client = useOperatorClient();
  const { state } = useOperatorAuth();
  const loader = useCallback(() => client.listOperatorRecoveryJobs({ limit: 25 }), [client]);
  const resource = useResource(loader);
  const [selected, setSelected] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  if (state.status !== "authenticated") return null;
  const job =
    resource.status === "ready"
      ? (resource.value.items.find((candidate) => candidate.id === selected) ?? null)
      : null;
  return (
    <section className="panel full-span">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Durable state machine</p>
          <h2>Recovery jobs</h2>
        </div>
        <ReloadButton reload={resource.reload} pending={resource.status === "loading"} />
      </div>
      <ResourceFailure resource={resource} />
      {message === null ? null : <p role="status">{message}</p>}
      {resource.status !== "ready" ? (
        <LoadingState label="Loading recovery jobs…" />
      ) : resource.value.items.length === 0 ? (
        <p>No recovery jobs are recorded.</p>
      ) : (
        <div className="operator-table-scroll">
          <table>
            <thead>
              <tr>
                <th scope="col">Job</th>
                <th scope="col">Project</th>
                <th scope="col">Backup</th>
                <th scope="col">State</th>
                <th scope="col">Verified</th>
                <th scope="col">Action</th>
              </tr>
            </thead>
            <tbody>
              {resource.value.items.map((candidate) => (
                <tr key={candidate.id}>
                  <td>
                    <code>{candidate.id}</code>
                  </td>
                  <td>
                    <code>{candidate.projectId}</code>
                  </td>
                  <td>{candidate.backupId}</td>
                  <td>{humanize(candidate.state)}</td>
                  <td>{candidate.verificationSucceeded ? "Yes" : "No"}</td>
                  <td>
                    <button
                      type="button"
                      className="secondary"
                      onClick={() => setSelected(candidate.id)}
                    >
                      Review
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {job === null ? null : (
        <div className="operator-recovery-detail">
          <h3>
            Reviewed recovery job <code>{job.id}</code>
          </h3>
          <dl className="operator-definition-grid">
            <div>
              <dt>Protected target</dt>
              <dd>{job.target}</dd>
            </div>
            <div>
              <dt>Version</dt>
              <dd>{job.version}</dd>
            </div>
            <div>
              <dt>Expires</dt>
              <dd>{formatDate(job.expiresAt)}</dd>
            </div>
            <div>
              <dt>Verification</dt>
              <dd>{job.verificationSucceeded ? "Succeeded" : "Pending"}</dd>
            </div>
          </dl>
          <p className="notice warning">
            Restore verification and promotion can run only through the approved server executor.
            The browser cannot claim verification or submit an arbitrary command or path.
          </p>
          {job.state === "promoted" ||
          job.state === "failed" ||
          job.state === "cancelled" ? null : (
            <button
              type="button"
              className="secondary"
              onClick={() => {
                void guardedInput(
                  "recovery_advance",
                  job.id,
                  job.version,
                  "Cancel reviewed recovery job without promotion",
                  state.session.passwordVerifiedAt,
                )
                  .then((guard) =>
                    client.advanceOperatorRecoveryJob(job.id, {
                      state: "cancelled",
                      verificationSucceeded: false,
                      errorClass: null,
                      guard,
                    }),
                  )
                  .then((updated) => setMessage(`Recovery job is now ${humanize(updated.state)}.`))
                  .then(resource.reload)
                  .catch((error: unknown) => setMessage(toConsoleApiFailure(error).message));
              }}
            >
              Cancel recovery job
            </button>
          )}
          {job.state === "promotion_ready" ? (
            <button
              type="button"
              disabled
              title="Approved recovery executor and promotion gate required"
            >
              Promotion confirmation unavailable until executor qualification
            </button>
          ) : null}
        </div>
      )}
    </section>
  );
}

function IncidentsPage({ permissions }: { readonly permissions: ReadonlySet<string> }) {
  const client = useOperatorClient();
  const [cursor, setCursor] = useState<string | undefined>();
  const [selected, setSelected] = useState<OperatorIncident | null>(null);
  const loader = useCallback(
    () => client.listOperatorIncidents({ ...(cursor === undefined ? {} : { cursor }), limit: 25 }),
    [client, cursor],
  );
  const resource = useResource(loader);
  const alertLoader = useCallback(() => client.listOperatorAlerts({ limit: 25 }), [client]);
  const alerts = useResource(alertLoader);
  return (
    <WorkspacePage
      title="Alerts and incidents"
      eyebrow="Durable response state"
      actions={<ReloadButton reload={resource.reload} pending={resource.status === "loading"} />}
    >
      <ResourceFailure resource={resource} />
      <ResourceFailure resource={alerts} />
      {alerts.status === "ready" ? <CurrentAlertTable value={alerts.value} /> : null}
      {permissions.has("incident_manage") ? (
        <CreateIncidentPanel
          onCreated={(incident) => {
            setSelected(incident);
            resource.reload();
          }}
        />
      ) : null}
      {resource.status !== "ready" ? (
        <LoadingState label="Loading incidents…" />
      ) : resource.value.items.length === 0 ? (
        <EmptyState title="No durable incidents are recorded" />
      ) : (
        <IncidentTable value={resource.value} onSelect={setSelected} onNext={setCursor} />
      )}
      {selected === null ? null : (
        <IncidentDetail
          incident={selected}
          canManage={permissions.has("incident_manage")}
          onChange={(incident) => {
            setSelected(incident);
            resource.reload();
          }}
        />
      )}
    </WorkspacePage>
  );
}

function CurrentAlertTable({ value }: { readonly value: OperatorAlertPage }) {
  return (
    <section className="panel full-span">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Observed state</p>
          <h2>Current alerts</h2>
        </div>
        <FreshnessBadge
          value={
            value.items.some((alert) => alert.freshness === "unavailable")
              ? "unavailable"
              : value.items.some((alert) => alert.freshness !== "current")
                ? "stale"
                : "current"
          }
        />
      </div>
      {value.items.length === 0 ? (
        <p>No current provider exceptions are visible.</p>
      ) : (
        <div className="operator-table-scroll">
          <table>
            <thead>
              <tr>
                <th scope="col">Fingerprint</th>
                <th scope="col">Severity</th>
                <th scope="col">Scope</th>
                <th scope="col">Freshness</th>
                <th scope="col">Observed</th>
                <th scope="col">Runbook</th>
              </tr>
            </thead>
            <tbody>
              {value.items.map((alert) => (
                <tr key={alert.fingerprint}>
                  <td>
                    <code>{alert.fingerprint}</code>
                  </td>
                  <td>{alert.severity}</td>
                  <td>{alert.affectedScope}</td>
                  <td>
                    <FreshnessBadge value={alert.freshness} />
                  </td>
                  <td>{formatDate(alert.lastObservedAt)}</td>
                  <td>
                    {alert.runbook === null ? (
                      "Not configured"
                    ) : (
                      <a href={alert.runbook.url} target="_blank" rel="noreferrer">
                        Open
                      </a>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function CreateIncidentPanel({
  onCreated,
}: {
  readonly onCreated: (value: OperatorIncident) => void;
}) {
  const client = useOperatorClient();
  const { state } = useOperatorAuth();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  if (state.status !== "authenticated") return null;
  return (
    <details className="panel operator-workflow-panel">
      <summary>Create incident</summary>
      <ApiFailureNotice failure={failure} />
      <form
        className="operator-form-grid"
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          const id = `inc_${crypto.randomUUID().replaceAll("-", "")}`;
          const reason = required(data, "reason");
          setPending(true);
          void guardedInput("incident_create", id, 0, reason, state.session.passwordVerifiedAt)
            .then((guard) =>
              client.createOperatorIncident({
                id,
                fingerprint: required(data, "fingerprint"),
                title: required(data, "title"),
                severity: required(data, "severity") as "critical" | "high" | "medium" | "low",
                projectId: optional(data, "projectId"),
                guard,
              }),
            )
            .then(onCreated)
            .then(() => setFailure(null))
            .catch((error: unknown) => setFailure(toConsoleApiFailure(error)))
            .finally(() => setPending(false));
        }}
      >
        <label>
          Title
          <input name="title" required maxLength={200} />
        </label>
        <label>
          Alert fingerprint
          <input name="fingerprint" required maxLength={128} />
        </label>
        <label>
          Severity
          <select name="severity" defaultValue="medium">
            <option>critical</option>
            <option>high</option>
            <option>medium</option>
            <option>low</option>
          </select>
        </label>
        <label>
          Optional project ID
          <input name="projectId" pattern="prj_[A-Za-z0-9_-]{8,64}" />
        </label>
        <label className="full-span">
          Reason / case context
          <textarea name="reason" required minLength={8} maxLength={1024} />
        </label>
        <button type="submit" disabled={pending}>
          {pending ? "Creating…" : "Review and create"}
        </button>
      </form>
    </details>
  );
}

function IncidentTable({
  value,
  onSelect,
  onNext,
}: {
  readonly value: OperatorIncidentPage;
  readonly onSelect: (incident: OperatorIncident) => void;
  readonly onNext: (cursor: string) => void;
}) {
  return (
    <>
      <div className="operator-table-scroll">
        <table>
          <thead>
            <tr>
              <th scope="col">Incident</th>
              <th scope="col">Severity</th>
              <th scope="col">State</th>
              <th scope="col">Updated</th>
              <th scope="col">Action</th>
            </tr>
          </thead>
          <tbody>
            {value.items.map((incident) => (
              <tr key={incident.id}>
                <td>
                  <strong>{incident.title}</strong>
                  <br />
                  <code>{incident.id}</code>
                </td>
                <td>{incident.severity}</td>
                <td>{humanize(incident.state)}</td>
                <td>{formatDate(incident.updatedAt)}</td>
                <td>
                  <button type="button" className="secondary" onClick={() => onSelect(incident)}>
                    Open
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {value.nextCursor === null ? null : (
        <button type="button" className="secondary" onClick={() => onNext(value.nextCursor ?? "")}>
          Next incident page
        </button>
      )}
    </>
  );
}

function IncidentDetail({
  incident,
  canManage,
  onChange,
}: {
  readonly incident: OperatorIncident;
  readonly canManage: boolean;
  readonly onChange: (incident: OperatorIncident) => void;
}) {
  const client = useOperatorClient();
  const { state } = useOperatorAuth();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  if (state.status !== "authenticated") return null;
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const action = required(data, "action") as "acknowledge" | "assign" | "annotate" | "resolve";
    const reason = required(data, "reason");
    setPending(true);
    void guardedInput(
      action,
      incident.id,
      incident.version,
      reason,
      state.session.passwordVerifiedAt,
    )
      .then((guard) =>
        client.updateOperatorIncident(incident.id, {
          action,
          assignee: optional(data, "assignee"),
          note: optional(data, "note"),
          guard,
        }),
      )
      .then(onChange)
      .then(() => setFailure(null))
      .catch((error: unknown) => setFailure(toConsoleApiFailure(error)))
      .finally(() => setPending(false));
  };
  return (
    <section className="panel operator-incident-detail" aria-labelledby="incident-detail-title">
      <p className="eyebrow">Version {incident.version}</p>
      <h2 id="incident-detail-title">{incident.title}</h2>
      <ApiFailureNotice failure={failure} />
      <ol className="operator-timeline">
        {incident.timeline.map((event) => (
          <li key={event.sequence}>
            <strong>{humanize(event.action)}</strong> by {event.actorId} at{" "}
            {formatDate(event.timestamp)}
            {event.note === null ? null : <p>{event.note}</p>}
          </li>
        ))}
      </ol>
      {!canManage || incident.state === "resolved" ? null : (
        <form className="operator-form-grid" onSubmit={submit}>
          <label>
            Action
            <select name="action">
              <option value="acknowledge">Acknowledge</option>
              <option value="assign">Assign</option>
              <option value="annotate">Annotate</option>
              <option value="resolve">Resolve</option>
            </select>
          </label>
          <label>
            Assignee
            <input name="assignee" maxLength={200} />
          </label>
          <label className="full-span">
            Timeline note
            <textarea name="note" maxLength={1024} />
          </label>
          <label className="full-span">
            Reason / case context
            <textarea name="reason" required minLength={8} maxLength={1024} />
          </label>
          <button type="submit" disabled={pending}>
            {pending ? "Applying…" : "Review and apply"}
          </button>
        </form>
      )}
    </section>
  );
}

function RecoveryRequestPanel() {
  const client = useOperatorClient();
  const { state } = useOperatorAuth();
  const [result, setResult] = useState<string | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  if (state.status !== "authenticated") return null;
  return (
    <details className="panel operator-workflow-panel">
      <summary>Request a gated recovery job</summary>
      <p>Creation and promotion have independent server-side feature gates.</p>
      <ApiFailureNotice failure={failure} />
      {result === null ? null : (
        <p role="status">
          Created recovery job <code>{result}</code>.
        </p>
      )}
      <form
        className="operator-form-grid"
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          const id = `rcv_${crypto.randomUUID().replaceAll("-", "")}`;
          const reason = required(data, "reason");
          setPending(true);
          void guardedInput("recovery_create", id, 0, reason, state.session.passwordVerifiedAt)
            .then((guard) => {
              const input: CreateOperatorRecoveryJobRequest = {
                id,
                projectId: required(data, "projectId"),
                backupId: required(data, "backupId"),
                target: required(data, "target"),
                backupVerified: data.get("backupVerified") === "on",
                impactPreview: required(data, "impactPreview"),
                guard,
              };
              return client.createOperatorRecoveryJob(input);
            })
            .then((job) => setResult(job.id))
            .then(() => setFailure(null))
            .catch((error: unknown) => setFailure(toConsoleApiFailure(error)))
            .finally(() => setPending(false));
        }}
      >
        <label>
          Project ID
          <input name="projectId" required pattern="prj_[A-Za-z0-9_-]{8,64}" />
        </label>
        <label>
          Verified backup ID
          <input name="backupId" required maxLength={128} />
        </label>
        <label>
          Protected restore target
          <input name="target" required maxLength={256} />
        </label>
        <label className="checkbox-row">
          <input type="checkbox" name="backupVerified" required /> Backup evidence reviewed and
          verified
        </label>
        <label className="full-span">
          Impact preview
          <textarea name="impactPreview" required maxLength={2048} />
        </label>
        <label className="full-span">
          Reason / case context
          <textarea name="reason" required minLength={8} maxLength={1024} />
        </label>
        <button type="submit" disabled={pending}>
          {pending ? "Requesting…" : "Review and request recovery"}
        </button>
      </form>
    </details>
  );
}

function ActivityPage({ permissions }: { readonly permissions: ReadonlySet<string> }) {
  const client = useOperatorClient();
  const [query, setQuery] = useState("");
  const [submitted, setSubmitted] = useState("");
  const loader = useCallback(
    () =>
      client.listOperatorActivity(
        submitted === "" ? { limit: 25 } : { query: submitted, limit: 25 },
      ),
    [client, submitted],
  );
  const resource = useResource(loader);
  return (
    <WorkspacePage
      title="Operator activity"
      eyebrow="Append-only projection"
      actions={<ReloadButton reload={resource.reload} pending={resource.status === "loading"} />}
    >
      <form
        className="operator-filter-bar"
        onSubmit={(event) => {
          event.preventDefault();
          setSubmitted(query.trim());
        }}
      >
        <label>
          Actor, action, target, tenant, case, request, or outcome
          <input
            type="search"
            value={query}
            maxLength={200}
            onChange={(event) => setQuery(event.currentTarget.value)}
          />
        </label>
        <button type="submit">Search</button>
      </form>
      <ResourceFailure resource={resource} />
      {permissions.has("activity_export") && resource.status === "ready" ? (
        <ActivityExportButton query={submitted} value={resource.value} />
      ) : null}
      {resource.status !== "ready" ? (
        <LoadingState label="Loading activity…" />
      ) : (
        <ActivityTable value={resource.value} />
      )}
    </WorkspacePage>
  );
}

const OPERATOR_PERMISSIONS: readonly OperatorPermission[] = [
  "tenant_read",
  "overview_read",
  "operations_read",
  "incident_read",
  "incident_manage",
  "backup_read",
  "recovery_manage",
  "fleet_read",
  "security_read",
  "security_manage",
  "activity_read",
  "activity_export",
  "provisioning_repair",
  "quota_override",
  "abuse_response",
  "support_access",
  "waitlist_review",
];

function SecurityPage({ permissions }: { readonly permissions: ReadonlySet<string> }) {
  const client = useOperatorClient();
  const loader = useCallback(() => client.getOperatorSecurityInventory(), [client]);
  const resource = useResource(loader);
  return (
    <WorkspacePage
      title="Operator security"
      eyebrow="Identity and access control"
      actions={<ReloadButton reload={resource.reload} pending={resource.status === "loading"} />}
    >
      <ResourceFailure resource={resource} />
      {resource.status !== "ready" ? (
        <LoadingState label="Loading bounded operator security state…" />
      ) : (
        <SecurityInventory
          value={resource.value}
          canManage={permissions.has("security_manage")}
          reload={resource.reload}
        />
      )}
      {permissions.has("security_manage") ? (
        <EntitlementAdministration onChanged={resource.reload} />
      ) : null}
    </WorkspacePage>
  );
}

function SecurityInventory({
  value,
  canManage,
  reload,
}: {
  readonly value: OperatorSecurityInventory;
  readonly canManage: boolean;
  readonly reload: () => void;
}) {
  const client = useOperatorClient();
  const [message, setMessage] = useState<string | null>(null);
  return (
    <>
      <div className="operator-stat-grid">
        <StatCard label="Entitlements" value={value.entitlements.length} tone="current" />
        <StatCard
          label="Active / recent sessions"
          value={value.sessions.filter((session) => session.revokedAt === null).length}
          tone="current"
        />
        <StatCard
          label="Authentication throttle records"
          value={value.attempts.length}
          tone={value.attempts.length > 0 ? "stale" : "current"}
        />
      </div>
      {message === null ? null : (
        <p role="status" className="notice">
          {message}
        </p>
      )}
      <div className="operator-table-scroll">
        <table>
          <thead>
            <tr>
              <th scope="col">Operator</th>
              <th scope="col">Developer identity</th>
              <th scope="col">Epoch</th>
              <th scope="col">Permissions</th>
              <th scope="col">Action</th>
            </tr>
          </thead>
          <tbody>
            {value.entitlements.map((entitlement) => (
              <tr key={entitlement.developerIdentityId}>
                <td>
                  <code>{entitlement.operatorId}</code>
                </td>
                <td>
                  <code>{entitlement.developerIdentityId}</code>
                </td>
                <td>{entitlement.operatorEpoch}</td>
                <td>{entitlement.permissions.map(humanize).join(", ")}</td>
                <td>
                  {!canManage ? null : (
                    <button
                      type="button"
                      className="secondary"
                      onClick={() => {
                        const reason = "Security administrator revoked active operator sessions";
                        void client
                          .revokeOperatorIdentitySessions(entitlement.developerIdentityId, reason)
                          .then((result) =>
                            setMessage(`${result.revokedSessions} active session(s) revoked.`),
                          )
                          .then(reload)
                          .catch((error: unknown) =>
                            setMessage(toConsoleApiFailure(error).message),
                          );
                      }}
                    >
                      Revoke sessions
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <section className="panel full-span">
        <h2>Authentication failures and throttling</h2>
        {value.attempts.length === 0 ? (
          <p>No retained attempt records.</p>
        ) : (
          <ul>
            {value.attempts.map((attempt) => (
              <li key={`${attempt.class}-${attempt.nextAllowedAt}-${attempt.count}`}>
                {humanize(attempt.class)}: {attempt.count} attempt(s), next allowed{" "}
                {formatRelative(attempt.nextAllowedAt)}
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

function EntitlementAdministration({ onChanged }: { readonly onChanged: () => void }) {
  const client = useOperatorClient();
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  return (
    <details className="panel operator-workflow-panel">
      <summary>Administer operator entitlement</summary>
      <p>
        The server plans and applies this change against the same digest. Long confirmation tokens
        are passed internally; you do not need to copy and paste one.
      </p>
      {message === null ? null : <p role="status">{message}</p>}
      <form
        className="operator-form-grid"
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          const kind = required(data, "kind") as "grant" | "replace" | "revoke";
          const selected = OPERATOR_PERMISSIONS.filter(
            (permission) => data.get(permission) === "on",
          );
          const input = {
            kind,
            targetEmail: required(data, "targetEmail"),
            permissions: kind === "revoke" ? [] : selected,
            privateReason: required(data, "privateReason"),
            environmentBinding: window.location.origin,
            idempotencyKey: `operator-entitlement-${crypto.randomUUID()}`,
          };
          setPending(true);
          void client
            .planOperatorEntitlementChange(input)
            .then((plan) => client.applyOperatorEntitlementChange(input, plan.typedConfirmation))
            .then((result) =>
              setMessage(
                `Entitlement is ${result.operatorStatusAfter} at epoch ${result.operatorEpoch}.`,
              ),
            )
            .then(onChanged)
            .catch((error: unknown) => setMessage(toConsoleApiFailure(error).message))
            .finally(() => setPending(false));
        }}
      >
        <label>
          Change
          <select name="kind">
            <option value="grant">Grant</option>
            <option value="replace">Replace</option>
            <option value="revoke">Revoke</option>
          </select>
        </label>
        <label>
          Developer email
          <input name="targetEmail" type="email" required maxLength={320} />
        </label>
        <fieldset className="full-span">
          <legend>Permissions (ignored for revoke)</legend>
          <div className="checkbox-grid">
            {OPERATOR_PERMISSIONS.map((permission) => (
              <label key={permission}>
                <input type="checkbox" name={permission} /> {humanize(permission)}
              </label>
            ))}
          </div>
        </fieldset>
        <label className="full-span">
          Private reason / case context
          <textarea name="privateReason" required minLength={8} maxLength={1024} />
        </label>
        <button type="submit" disabled={pending}>
          {pending ? "Planning and applying…" : "Review and apply"}
        </button>
      </form>
    </details>
  );
}

function ActivityTable({ value }: { readonly value: OperatorActivityPage }) {
  if (value.items.length === 0) return <EmptyState title="No activity matches these filters" />;
  return (
    <div className="operator-table-scroll">
      <table>
        <thead>
          <tr>
            <th scope="col">Time</th>
            <th scope="col">Actor</th>
            <th scope="col">Action</th>
            <th scope="col">Target</th>
            <th scope="col">Outcome</th>
            <th scope="col">Integrity</th>
          </tr>
        </thead>
        <tbody>
          {value.items.map((event) => (
            <tr key={event.id}>
              <td>{formatDate(event.timestamp)}</td>
              <td>{event.actorId}</td>
              <td>{humanize(event.action)}</td>
              <td>
                <code>{event.target}</code>
              </td>
              <td>{event.outcome}</td>
              <td>
                <code>{event.integrity.slice(0, 12)}…</code>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ActivityExportButton({
  query,
  value,
}: {
  readonly query: string;
  readonly value: OperatorActivityPage;
}) {
  const client = useOperatorClient();
  const { state } = useOperatorAuth();
  const [message, setMessage] = useState<string | null>(null);
  if (state.status !== "authenticated") return null;
  return (
    <div className="operator-export-row">
      <button
        type="button"
        className="secondary"
        onClick={() => {
          const id = `exp_${crypto.randomUUID().replaceAll("-", "")}`;
          void guardedInput(
            "activity_export",
            id,
            0,
            "Export operator activity for reviewed filters",
            state.session.passwordVerifiedAt,
          )
            .then((guard) =>
              client.createOperatorActivityExport({
                id,
                filter: { query, nextCursor: value.nextCursor },
                guard,
              }),
            )
            .then((record) => client.processOperatorActivityExport(record.id))
            .then((record) =>
              setMessage(
                `Export ${record.id} is ${record.state}, contains ${record.recordCount} records, checksum ${record.checksum?.slice(0, 12) ?? "pending"}…, and expires ${formatRelative(record.expiresAt)}.`,
              ),
            )
            .catch((error: unknown) => setMessage(toConsoleApiFailure(error).message));
        }}
      >
        Create expiring export
      </button>
      {message === null ? null : <span role="status">{message}</span>}
    </div>
  );
}

function WorkspacePage({
  title,
  eyebrow,
  actions,
  children,
}: {
  readonly title: string;
  readonly eyebrow: string;
  readonly actions?: ReactNode;
  readonly children: ReactNode;
}) {
  return (
    <section className="operator-page" aria-labelledby="operator-page-title">
      <div className="operator-page-heading">
        <div>
          <p className="eyebrow">{eyebrow}</p>
          <h1 id="operator-page-title">{title}</h1>
        </div>
        {actions}
      </div>
      {children}
    </section>
  );
}

function SectionGrid({ sections }: { readonly sections: readonly OperatorReadSection[] }) {
  if (sections.length === 0) return <EmptyState title="No provider sections are configured" />;
  return (
    <div className="operator-section-grid">
      {sections.map((section) => (
        <section className="panel operator-provider-card" key={section.id}>
          <div className="section-heading">
            <div>
              <p className="eyebrow">{section.provider}</p>
              <h2>{humanize(section.id)}</h2>
            </div>
            <FreshnessBadge value={section.freshness} />
          </div>
          {section.message === null ? null : <p>{section.message}</p>}
          {Object.keys(section.metrics).length === 0 ? null : (
            <dl className="operator-metric-list">
              {Object.entries(section.metrics).map(([name, value]) => (
                <div key={name}>
                  <dt>{humanize(name)}</dt>
                  <dd>{metricValue(value)}</dd>
                </div>
              ))}
            </dl>
          )}
          {section.links.length === 0 ? null : (
            <ul>
              {section.links.map((link) => (
                <li key={link.url}>
                  <a href={link.url} target="_blank" rel="noreferrer">
                    {link.label}
                  </a>
                </li>
              ))}
            </ul>
          )}
          <small>
            Observed {section.observedAt === null ? "unknown" : formatRelative(section.observedAt)}
          </small>
        </section>
      ))}
    </div>
  );
}

function FreshnessBadge({ value }: { readonly value: string }) {
  return <span className={`status-pill ${value}`}>{humanize(value)}</span>;
}

function StatCard({
  label,
  value,
  tone,
}: {
  readonly label: string;
  readonly value: number | string;
  readonly tone: string;
}) {
  return (
    <section className="panel operator-stat">
      <span>{label}</span>
      <strong>{value}</strong>
      <span className={`status-dot ${tone}`} aria-hidden="true" />
    </section>
  );
}

function EmptyState({ title }: { readonly title: string }) {
  return (
    <section className="panel operator-empty-state">
      <h2>{title}</h2>
      <p>Adjust filters or refresh after the underlying state changes.</p>
    </section>
  );
}

function LoadingState({ label }: { readonly label: string }) {
  return (
    <p className="panel" aria-live="polite" aria-busy="true">
      {label}
    </p>
  );
}

function ReloadButton({
  reload,
  pending,
}: {
  readonly reload: () => void;
  readonly pending: boolean;
}) {
  return (
    <button type="button" className="secondary" disabled={pending} onClick={reload}>
      {pending ? "Refreshing…" : "Refresh"}
    </button>
  );
}

function CursorControls({
  canPrevious,
  canNext,
  onPrevious,
  onNext,
}: {
  readonly canPrevious: boolean;
  readonly canNext: boolean;
  readonly onPrevious: () => void;
  readonly onNext: () => void;
}) {
  return (
    <nav className="button-row spread" aria-label="Cursor pagination">
      <button type="button" className="secondary" disabled={!canPrevious} onClick={onPrevious}>
        Previous page
      </button>
      <span>Up to 25 records per page</span>
      <button type="button" className="secondary" disabled={!canNext} onClick={onNext}>
        Next page
      </button>
    </nav>
  );
}

type ResourceState<T> =
  | { readonly status: "loading" }
  | { readonly status: "error"; readonly failure: ConsoleApiFailure }
  | { readonly status: "ready"; readonly value: T };

type Resource<T> = ResourceState<T> & { readonly reload: () => void };

function useResource<T>(load: () => Promise<T>): Resource<T> {
  const [version, setVersion] = useState(0);
  const [state, setState] = useState<ResourceState<T>>({ status: "loading" });
  useEffect(() => {
    const reloadVersion = version;
    let live = true;
    setState({ status: "loading" });
    load().then(
      (value) => {
        if (live && reloadVersion === version) setState({ status: "ready", value });
      },
      (error: unknown) => {
        if (live && reloadVersion === version)
          setState({ status: "error", failure: toConsoleApiFailure(error) });
      },
    );
    return () => {
      live = false;
    };
  }, [load, version]);
  const reload = useCallback(() => setVersion((value) => value + 1), []);
  return useMemo(() => ({ ...state, reload }) as Resource<T>, [state, reload]);
}

function ResourceFailure<T>({ resource }: { readonly resource: Resource<T> }) {
  return resource.status === "error" ? <ApiFailureNotice failure={resource.failure} /> : null;
}

function inventoryForSection(section: OperatorSection): {
  readonly kind: OperatorInventoryKind;
  readonly title: string;
  readonly description: string;
} | null {
  if (section === "operations")
    return {
      kind: "operations",
      title: "Provisioning and operations",
      description:
        "Failed, stalled, and pending work with safe error classes and contextual repair paths.",
    };
  if (section === "backups")
    return {
      kind: "backups",
      title: "Backups and recovery",
      description:
        "Backup age, integrity, remote verification, restore drills, and gated recovery objectives.",
    };
  if (section === "sync")
    return {
      kind: "sync",
      title: "RxDB synchronization",
      description:
        "Live streams, pull and push outcomes, lag, conflicts, policy denials, checkpoints, and resynchronization.",
    };
  if (section === "fleet")
    return {
      kind: "fleet",
      title: "Service fleet",
      description:
        "Region, version, readiness, restarts, dependencies, certificate expiry, and configuration drift.",
    };
  if (section === "storage")
    return {
      kind: "rocksdb",
      title: "RocksDB volumes",
      description:
        "Capacity, stalls, compaction, background errors, corruption evidence, backups, and recovery readiness.",
    };
  return null;
}

function allowsRead(permissions: ReadonlySet<string>, permission: string): boolean {
  return (
    permissions.has(permission) ||
    (permissions.has("tenant_read") &&
      [
        "overview_read",
        "operations_read",
        "incident_read",
        "backup_read",
        "fleet_read",
        "security_read",
        "activity_read",
      ].includes(permission))
  );
}

async function guardedInput(
  action: string,
  target: string,
  reviewedVersion: number,
  reason: string,
  passwordVerifiedAt: string,
) {
  const content = new TextEncoder().encode(
    `operator-action-v1\0${action}\0${target}\0${reviewedVersion}`,
  );
  const digest = await crypto.subtle.digest("SHA-256", content);
  const actionBinding = [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
  return {
    operationKey: `operator-${action}-${crypto.randomUUID()}`,
    reviewedVersion,
    reason,
    caseReference: null,
    confirmation: target,
    actionBinding,
    passwordVerifiedAt,
  };
}

function required(data: FormData, name: string): string {
  const value = String(data.get(name) ?? "").trim();
  if (value === "") throw new Error(`${name} is required.`);
  return value;
}

function optional(data: FormData, name: string): string | null {
  const value = String(data.get(name) ?? "").trim();
  return value === "" ? null : value;
}

function metricValue(value: unknown): string {
  if (value === null) return "Unknown";
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean")
    return String(value);
  return JSON.stringify(value).slice(0, 256);
}

function humanize(value: string): string {
  return value.replaceAll(/([a-z])([A-Z])/gu, "$1 $2").replaceAll("_", " ");
}

function formatDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString();
}

function formatRelative(value: string): string {
  const time = Date.parse(value);
  if (!Number.isFinite(time)) return "at an unknown time";
  const seconds = Math.round((time - Date.now()) / 1_000);
  const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  if (Math.abs(seconds) < 120) return formatter.format(seconds, "second");
  const minutes = Math.round(seconds / 60);
  if (Math.abs(minutes) < 120) return formatter.format(minutes, "minute");
  const hours = Math.round(minutes / 60);
  return formatter.format(hours, "hour");
}
