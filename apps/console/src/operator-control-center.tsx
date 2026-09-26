import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Checkbox,
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
  Textarea,
  ThemeToggle,
  cn,
  useTheme,
} from "@mako-cloud/ui";
import {
  Activity,
  ArrowLeft,
  Building2,
  ChevronLeft,
  ChevronRight,
  DatabaseBackup,
  ExternalLink,
  FolderSync,
  HardDrive,
  Inbox,
  LayoutDashboard,
  LoaderCircle,
  type LucideIcon,
  RefreshCw,
  Search,
  Server,
  ShieldCheck,
  Siren,
  UserCheck,
  Wrench,
} from "lucide-react";
import {
  type FormEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from "react";

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
  readonly icon: LucideIcon;
}

const NAVIGATION: readonly NavigationItem[] = [
  { section: "overview", label: "Overview", permission: "overview_read", icon: LayoutDashboard },
  { section: "tenants", label: "Tenants", permission: "tenant_read", icon: Building2 },
  { section: "operations", label: "Operations", permission: "operations_read", icon: Wrench },
  { section: "incidents", label: "Incidents", permission: "incident_read", icon: Siren },
  {
    section: "backups",
    label: "Backups & recovery",
    permission: "backup_read",
    icon: DatabaseBackup,
  },
  { section: "sync", label: "RxDB sync", permission: "fleet_read", icon: FolderSync },
  { section: "fleet", label: "Fleet", permission: "fleet_read", icon: Server },
  { section: "storage", label: "Storage", permission: "fleet_read", icon: HardDrive },
  { section: "security", label: "Security", permission: "security_read", icon: ShieldCheck },
  { section: "activity", label: "Activity", permission: "activity_read", icon: Activity },
  {
    section: "waitlist",
    label: "Developer wait list",
    permission: "waitlist_review",
    icon: UserCheck,
  },
];

/**
 * The same per-device key the developer console uses, so a person who flips
 * the theme on one surface finds the other already following it.
 */
const OPERATOR_THEME_KEY = "mako.console.theme";

export function OperatorWorkspaceScreen({
  section,
  projectId,
  navigate,
  onExit,
}: OperatorWorkspaceProps) {
  const { state, signOut } = useOperatorAuth();
  const { resolved, toggle } = useTheme(OPERATOR_THEME_KEY);
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
    <div className="flex min-h-screen flex-col bg-background text-foreground">
      <a
        className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-card focus:px-3 focus:py-2 focus:shadow-md"
        href="#operator-main-content"
      >
        Skip to operator workspace
      </a>
      <header className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b bg-card px-4 py-3 sm:px-6">
        <div className="flex min-w-0 items-center gap-3">
          <ShieldCheck aria-hidden="true" className="size-5 shrink-0 text-primary" />
          <div className="grid min-w-0 leading-tight">
            <Eyebrow>Audited platform administration</Eyebrow>
            <strong className="truncate text-sm">Mako Cloud Control Center</strong>
          </div>
          <Badge variant="outline" className="border-warning/50 bg-warning/10">
            Operator
          </Badge>
        </div>
        <div className="ml-auto flex flex-wrap items-center gap-x-3 gap-y-2 text-sm text-muted-foreground">
          <span className="max-w-56 truncate">{session.profile.email}</span>
          <span title={session.expiresAt}>Session expires {formatRelative(session.expiresAt)}</span>
          <SupportModeStatus />
          <div className="flex items-center gap-2">
            <ThemeToggle resolved={resolved} onToggle={toggle} data-testid="theme-toggle" />
            <Button variant="outline" size="sm" onClick={onExit}>
              Developer site
            </Button>
            <Button size="sm" onClick={() => void signOut()}>
              Sign out
            </Button>
          </div>
        </div>
      </header>
      <div className="flex flex-1 flex-col md:grid md:grid-cols-[15rem_minmax(0,1fr)]">
        <aside
          className="border-b bg-sidebar text-sidebar-foreground md:border-r md:border-b-0"
          aria-label="Operator workspace"
        >
          <div className="flex flex-col gap-4 px-3 py-3 md:sticky md:top-0 md:max-h-screen md:overflow-y-auto md:py-5">
            <nav className="flex gap-1 overflow-x-auto md:grid md:overflow-visible">
              {visibleNavigation.map((item) => {
                const active = selected === item.section;
                const Icon = item.icon;
                return (
                  <Button
                    key={item.section}
                    variant="ghost"
                    className={cn(
                      "h-9 shrink-0 justify-start gap-3 px-3 font-medium text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground md:w-full",
                      active && "bg-sidebar-accent text-sidebar-foreground",
                    )}
                    aria-current={active ? "page" : undefined}
                    onClick={() => navigate(`/operator/${item.section}`)}
                  >
                    <Icon aria-hidden="true" className="size-4 shrink-0" />
                    {item.label}
                  </Button>
                );
              })}
            </nav>
            <p className="m-0 border-t px-3 pt-4 text-xs leading-relaxed text-muted-foreground">
              Routine pages expose metadata and aggregate signals only. Document content requires a
              separately verified, scoped support session.
            </p>
          </div>
        </aside>
        <main
          className="min-w-0 flex-1 px-4 py-6 outline-none sm:px-6 lg:px-8"
          id="operator-main-content"
          tabIndex={-1}
        >
          <OperatorRoute
            section={effectiveSection}
            projectId={projectId}
            navigate={navigate}
            permissions={permissions}
          />
        </main>
      </div>
    </div>
  );
}

function SupportModeStatus() {
  const client = useOperatorClient();
  const loader = useCallback(() => client.listCurrentOperatorSupportSessions(25), [client]);
  const resource = useResource(loader);
  if (resource.status === "loading") {
    return (
      <Badge variant="outline" className="text-muted-foreground">
        Support mode: checking
      </Badge>
    );
  }
  if (resource.status === "error") {
    return <Badge variant="destructive">Support mode: unknown</Badge>;
  }
  if (resource.value.length === 0) {
    return <Badge variant="secondary">Support mode: inactive</Badge>;
  }
  const scopes = resource.value.map((grant) =>
    grant.environmentId === null ? grant.projectId : `${grant.projectId}/${grant.environmentId}`,
  );
  return (
    <Badge variant="warning" title={scopes.join(", ")}>
      Support mode: active ({resource.value.length})
    </Badge>
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
      <Alert variant="destructive" role="alert">
        <AlertDescription className="block">
          This route is not available to the current operator entitlement.
        </AlertDescription>
      </Alert>
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
          <div className="grid gap-4 sm:grid-cols-3">
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
  const id = useId();
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
      <form className="flex flex-wrap items-end gap-3" onSubmit={submit}>
        <Field
          label="Team, project, environment, region, developer email, or identifier"
          htmlFor={`${id}-query`}
          className="min-w-0 flex-1 basis-72"
        >
          <Input
            id={`${id}-query`}
            type="search"
            value={query}
            maxLength={200}
            onChange={(event) => setQuery(event.currentTarget.value)}
          />
        </Field>
        <Button type="submit">
          <Search aria-hidden="true" />
          Search
        </Button>
      </form>
      <ResourceFailure resource={resource} />
      {resource.status !== "ready" ? (
        <LoadingState label="Loading a bounded tenant page…" />
      ) : resource.value.items.length === 0 ? (
        <WorkspaceEmptyState title="No tenants match this bounded page" />
      ) : (
        <>
          <Card className="gap-0 overflow-hidden py-0">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col" className="pl-4">
                    Project
                  </TableHead>
                  <TableHead scope="col">Team</TableHead>
                  <TableHead scope="col">Lifecycle</TableHead>
                  <TableHead scope="col">Region</TableHead>
                  <TableHead scope="col">Environments</TableHead>
                  <TableHead scope="col">Health</TableHead>
                  <TableHead scope="col" className="pr-4 text-right">
                    Action
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {resource.value.items.map((tenant) => (
                  <TableRow key={tenant.projectId}>
                    <TableCell className="pl-4">
                      <div className="grid gap-0.5">
                        <strong className="font-medium">{tenant.projectName}</strong>
                        <code className="font-mono text-xs text-muted-foreground">
                          {tenant.projectId}
                        </code>
                      </div>
                    </TableCell>
                    <TableCell>{tenant.teamName ?? tenant.teamId}</TableCell>
                    <TableCell>
                      <LifecycleBadge value={tenant.lifecycle} />
                    </TableCell>
                    <TableCell className="text-muted-foreground">{tenant.region}</TableCell>
                    <TableCell className="tabular-nums">{tenant.environmentCount}</TableCell>
                    <TableCell>
                      <FreshnessBadge value={tenant.health} />
                    </TableCell>
                    <TableCell className="pr-4 text-right">
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => navigate(`/operator/tenants/${tenant.projectId}`)}
                      >
                        Open Tenant 360
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </Card>
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
      eyebrow={
        <span className="font-mono font-medium tracking-normal normal-case">{projectId}</span>
      }
      actions={
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" onClick={() => navigate("/operator/tenants")}>
            <ArrowLeft aria-hidden="true" />
            Back to tenants
          </Button>
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
      <Card>
        <CardHeader>
          <Eyebrow>{value.project.teamName ?? value.project.teamId}</Eyebrow>
          <CardTitle className="text-xl">{value.project.projectName}</CardTitle>
          <code className="font-mono text-xs text-muted-foreground">{value.project.projectId}</code>
        </CardHeader>
        <CardContent>
          <dl className="m-0 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <Definition term="Lifecycle">
              <LifecycleBadge value={value.project.lifecycle} />
            </Definition>
            <Definition term="Region">{value.project.region}</Definition>
            <Definition term="Plan">{value.project.plan}</Definition>
            <Definition term="Health">
              <FreshnessBadge value={value.project.health} />
            </Definition>
          </dl>
        </CardContent>
      </Card>
      {value.partial ? (
        <Alert variant="warning" role="status">
          <AlertDescription className="block">
            One or more providers are unavailable. Successful sections remain visible and
            unavailable sections do not imply healthy state.
          </AlertDescription>
        </Alert>
      ) : null}
      <Card>
        <CardHeader>
          <CardTitle>Topology</CardTitle>
        </CardHeader>
        <CardContent>
          {value.environments.length === 0 ? (
            <p className="m-0 text-sm text-muted-foreground">No environments exist.</p>
          ) : (
            <ul className="m-0 grid list-none gap-2 p-0">
              {value.environments.map((environment) => (
                <li key={environment.id} className="flex flex-wrap items-center gap-2 text-sm">
                  <strong>{environment.name}</strong>{" "}
                  <code className="font-mono text-xs text-muted-foreground">{environment.id}</code>{" "}
                  <LifecycleBadge value={environment.state} />
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>
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
  const id = useId();
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
      subtitle={description}
      actions={<ReloadButton reload={resource.reload} pending={resource.status === "loading"} />}
    >
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          setScope(projectId.trim() === "" ? undefined : projectId.trim());
        }}
      >
        <Field
          label="Optional project scope"
          htmlFor={`${id}-scope`}
          className="min-w-0 flex-1 basis-72 sm:max-w-md"
        >
          <Input
            id={`${id}-scope`}
            value={projectId}
            pattern="prj_[A-Za-z0-9_\-]{8,64}"
            placeholder="prj_…"
            className="font-mono"
            onChange={(event) => setProjectId(event.currentTarget.value)}
          />
        </Field>
        <Button type="submit">Apply scope</Button>
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
      <Card>
        <CardHeader>
          <CardTitle>Contextual workflows</CardTitle>
          <CardDescription>
            Apply an exact project scope above before loading or changing provisioning, quota,
            abuse, or support state.
          </CardDescription>
        </CardHeader>
      </Card>
    );
  return (
    <section className="grid gap-4" aria-label="Contextual operational workflows">
      <ApiFailureNotice failure={failure} />
      {resource.status !== "ready" || resource.value === null ? (
        <LoadingState label="Loading exact-project workflow history…" />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>Selected tenant workflow inventory</CardTitle>
              <CardDescription>
                <code className="font-mono text-xs text-foreground">{projectId}</code> ·{" "}
                {resource.value.workflows.length} provisioning · {resource.value.quotas.length}{" "}
                quota · {resource.value.abuseResponses.length} abuse ·{" "}
                {resource.value.supportSessions.length} support records.
              </CardDescription>
            </CardHeader>
          </Card>
          <div className="grid gap-4 lg:grid-cols-2">
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
    <Card>
      <CardHeader>
        <Eyebrow>Durable state machine</Eyebrow>
        <CardTitle>Recovery jobs</CardTitle>
        <CardAction>
          <ReloadButton reload={resource.reload} pending={resource.status === "loading"} />
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ResourceFailure resource={resource} />
        {message === null ? null : (
          <Alert role="status">
            <AlertDescription className="block text-foreground">{message}</AlertDescription>
          </Alert>
        )}
        {resource.status !== "ready" ? (
          <LoadingState label="Loading recovery jobs…" />
        ) : resource.value.items.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">No recovery jobs are recorded.</p>
        ) : (
          <div className="overflow-hidden rounded-lg border">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col" className="pl-3">
                    Job
                  </TableHead>
                  <TableHead scope="col">Project</TableHead>
                  <TableHead scope="col">Backup</TableHead>
                  <TableHead scope="col">State</TableHead>
                  <TableHead scope="col">Verified</TableHead>
                  <TableHead scope="col" className="pr-3 text-right">
                    Action
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {resource.value.items.map((candidate) => (
                  <TableRow key={candidate.id}>
                    <TableCell className="pl-3">
                      <code className="font-mono text-xs">{candidate.id}</code>
                    </TableCell>
                    <TableCell>
                      <code className="font-mono text-xs">{candidate.projectId}</code>
                    </TableCell>
                    <TableCell>{candidate.backupId}</TableCell>
                    <TableCell>
                      <Badge variant="secondary">{humanize(candidate.state)}</Badge>
                    </TableCell>
                    <TableCell>{candidate.verificationSucceeded ? "Yes" : "No"}</TableCell>
                    <TableCell className="pr-3 text-right">
                      <Button variant="outline" size="sm" onClick={() => setSelected(candidate.id)}>
                        Review
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
        {job === null ? null : (
          <div className="grid gap-4 border-t pt-5">
            <h3 className="text-base">
              Reviewed recovery job <code className="font-mono text-sm">{job.id}</code>
            </h3>
            <dl className="m-0 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
              <Definition term="Protected target">{job.target}</Definition>
              <Definition term="Version">
                <span className="tabular-nums">{job.version}</span>
              </Definition>
              <Definition term="Expires">{formatDate(job.expiresAt)}</Definition>
              <Definition term="Verification">
                {job.verificationSucceeded ? "Succeeded" : "Pending"}
              </Definition>
            </dl>
            <Alert variant="warning" role="note">
              <AlertDescription className="block">
                Restore verification and promotion can run only through the approved server
                executor. The browser cannot claim verification or submit an arbitrary command or
                path.
              </AlertDescription>
            </Alert>
            <div className="flex flex-wrap gap-2">
              {job.state === "promoted" ||
              job.state === "failed" ||
              job.state === "cancelled" ? null : (
                <Button
                  variant="outline"
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
                      .then((updated) =>
                        setMessage(`Recovery job is now ${humanize(updated.state)}.`),
                      )
                      .then(resource.reload)
                      .catch((error: unknown) => setMessage(toConsoleApiFailure(error).message));
                  }}
                >
                  Cancel recovery job
                </Button>
              )}
              {job.state === "promotion_ready" ? (
                <Button disabled title="Approved recovery executor and promotion gate required">
                  Promotion confirmation unavailable until executor qualification
                </Button>
              ) : null}
            </div>
          </div>
        )}
      </CardContent>
    </Card>
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
        <WorkspaceEmptyState title="No durable incidents are recorded" />
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
    <Card className="overflow-hidden pb-0">
      <CardHeader>
        <Eyebrow>Observed state</Eyebrow>
        <CardTitle>Current alerts</CardTitle>
        <CardAction>
          <FreshnessBadge
            value={
              value.items.some((alert) => alert.freshness === "unavailable")
                ? "unavailable"
                : value.items.some((alert) => alert.freshness !== "current")
                  ? "stale"
                  : "current"
            }
          />
        </CardAction>
      </CardHeader>
      {value.items.length === 0 ? (
        <CardContent className="pb-5">
          <p className="m-0 text-sm text-muted-foreground">
            No current provider exceptions are visible.
          </p>
        </CardContent>
      ) : (
        <div className="border-t">
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col" className="pl-5">
                  Fingerprint
                </TableHead>
                <TableHead scope="col">Severity</TableHead>
                <TableHead scope="col">Scope</TableHead>
                <TableHead scope="col">Freshness</TableHead>
                <TableHead scope="col">Observed</TableHead>
                <TableHead scope="col" className="pr-5">
                  Runbook
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {value.items.map((alert) => (
                <TableRow key={alert.fingerprint}>
                  <TableCell className="pl-5">
                    <code className="font-mono text-xs">{alert.fingerprint}</code>
                  </TableCell>
                  <TableCell>
                    <SeverityBadge value={alert.severity} />
                  </TableCell>
                  <TableCell>{alert.affectedScope}</TableCell>
                  <TableCell>
                    <FreshnessBadge value={alert.freshness} />
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {formatDate(alert.lastObservedAt)}
                  </TableCell>
                  <TableCell className="pr-5">
                    {alert.runbook === null ? (
                      <span className="text-muted-foreground">Not configured</span>
                    ) : (
                      <a
                        className="inline-flex items-center gap-1"
                        href={alert.runbook.url}
                        target="_blank"
                        rel="noreferrer"
                      >
                        Open
                        <ExternalLink aria-hidden="true" className="size-3.5" />
                      </a>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </Card>
  );
}

function CreateIncidentPanel({
  onCreated,
}: {
  readonly onCreated: (value: OperatorIncident) => void;
}) {
  const client = useOperatorClient();
  const { state } = useOperatorAuth();
  const id = useId();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  if (state.status !== "authenticated") return null;
  return (
    <Disclosure summary="Create incident">
      <ApiFailureNotice failure={failure} />
      <form
        className="grid gap-4 sm:grid-cols-2"
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          const incidentId = `inc_${crypto.randomUUID().replaceAll("-", "")}`;
          const reason = required(data, "reason");
          setPending(true);
          void guardedInput(
            "incident_create",
            incidentId,
            0,
            reason,
            state.session.passwordVerifiedAt,
          )
            .then((guard) =>
              client.createOperatorIncident({
                id: incidentId,
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
        <Field label="Title" htmlFor={`${id}-title`}>
          <Input id={`${id}-title`} name="title" required maxLength={200} />
        </Field>
        <Field label="Alert fingerprint" htmlFor={`${id}-fingerprint`}>
          <Input
            id={`${id}-fingerprint`}
            name="fingerprint"
            required
            maxLength={128}
            className="font-mono"
          />
        </Field>
        <Field label="Severity" htmlFor={`${id}-severity`}>
          <NativeSelect id={`${id}-severity`} name="severity" defaultValue="medium">
            <option>critical</option>
            <option>high</option>
            <option>medium</option>
            <option>low</option>
          </NativeSelect>
        </Field>
        <Field label="Optional project ID" htmlFor={`${id}-project`}>
          <Input
            id={`${id}-project`}
            name="projectId"
            pattern="prj_[A-Za-z0-9_\-]{8,64}"
            className="font-mono"
          />
        </Field>
        <Field label="Reason / case context" htmlFor={`${id}-reason`} className="sm:col-span-2">
          <Textarea id={`${id}-reason`} name="reason" required minLength={8} maxLength={1024} />
        </Field>
        <div className="sm:col-span-2">
          <Button type="submit" disabled={pending}>
            {pending ? "Creating…" : "Review and create"}
          </Button>
        </div>
      </form>
    </Disclosure>
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
      <Card className="gap-0 overflow-hidden py-0">
        <Table>
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead scope="col" className="pl-4">
                Incident
              </TableHead>
              <TableHead scope="col">Severity</TableHead>
              <TableHead scope="col">State</TableHead>
              <TableHead scope="col">Updated</TableHead>
              <TableHead scope="col" className="pr-4 text-right">
                Action
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {value.items.map((incident) => (
              <TableRow key={incident.id}>
                <TableCell className="pl-4">
                  <div className="grid gap-0.5">
                    <strong className="font-medium">{incident.title}</strong>
                    <code className="font-mono text-xs text-muted-foreground">{incident.id}</code>
                  </div>
                </TableCell>
                <TableCell>
                  <SeverityBadge value={incident.severity} />
                </TableCell>
                <TableCell>
                  <Badge variant="secondary">{humanize(incident.state)}</Badge>
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {formatDate(incident.updatedAt)}
                </TableCell>
                <TableCell className="pr-4 text-right">
                  <Button variant="outline" size="sm" onClick={() => onSelect(incident)}>
                    Open
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </Card>
      {value.nextCursor === null ? null : (
        <div className="flex justify-end">
          <Button variant="outline" size="sm" onClick={() => onNext(value.nextCursor ?? "")}>
            Next incident page
            <ChevronRight aria-hidden="true" />
          </Button>
        </div>
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
  const id = useId();
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
    <Card aria-labelledby="incident-detail-title">
      <CardHeader>
        <Eyebrow>Version {incident.version}</Eyebrow>
        <CardTitle id="incident-detail-title">{incident.title}</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-5">
        <ApiFailureNotice failure={failure} />
        <ol className="m-0 grid list-none gap-3 p-0">
          {incident.timeline.map((event) => (
            <li key={event.sequence} className="border-l-2 pl-4 text-sm">
              <strong>{humanize(event.action)}</strong> by {event.actorId} at{" "}
              {formatDate(event.timestamp)}
              {event.note === null ? null : (
                <p className="m-0 mt-1 text-muted-foreground">{event.note}</p>
              )}
            </li>
          ))}
        </ol>
        {!canManage || incident.state === "resolved" ? null : (
          <form className="grid gap-4 border-t pt-5 sm:grid-cols-2" onSubmit={submit}>
            <Field label="Action" htmlFor={`${id}-action`}>
              <NativeSelect id={`${id}-action`} name="action">
                <option value="acknowledge">Acknowledge</option>
                <option value="assign">Assign</option>
                <option value="annotate">Annotate</option>
                <option value="resolve">Resolve</option>
              </NativeSelect>
            </Field>
            <Field label="Assignee" htmlFor={`${id}-assignee`}>
              <Input id={`${id}-assignee`} name="assignee" maxLength={200} />
            </Field>
            <Field label="Timeline note" htmlFor={`${id}-note`} className="sm:col-span-2">
              <Textarea id={`${id}-note`} name="note" maxLength={1024} />
            </Field>
            <Field label="Reason / case context" htmlFor={`${id}-reason`} className="sm:col-span-2">
              <Textarea id={`${id}-reason`} name="reason" required minLength={8} maxLength={1024} />
            </Field>
            <div className="sm:col-span-2">
              <Button type="submit" disabled={pending}>
                {pending ? "Applying…" : "Review and apply"}
              </Button>
            </div>
          </form>
        )}
      </CardContent>
    </Card>
  );
}

function RecoveryRequestPanel() {
  const client = useOperatorClient();
  const { state } = useOperatorAuth();
  const id = useId();
  const [result, setResult] = useState<string | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  if (state.status !== "authenticated") return null;
  return (
    <Disclosure summary="Request a gated recovery job">
      <p className="m-0 text-sm text-muted-foreground">
        Creation and promotion have independent server-side feature gates.
      </p>
      <ApiFailureNotice failure={failure} />
      {result === null ? null : (
        <Alert role="status">
          <AlertDescription className="block text-foreground">
            Created recovery job <code className="font-mono text-xs">{result}</code>.
          </AlertDescription>
        </Alert>
      )}
      <form
        className="grid gap-4 sm:grid-cols-2"
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          const jobId = `rcv_${crypto.randomUUID().replaceAll("-", "")}`;
          const reason = required(data, "reason");
          setPending(true);
          void guardedInput("recovery_create", jobId, 0, reason, state.session.passwordVerifiedAt)
            .then((guard) => {
              const input: CreateOperatorRecoveryJobRequest = {
                id: jobId,
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
        <Field label="Project ID" htmlFor={`${id}-project`}>
          <Input
            id={`${id}-project`}
            name="projectId"
            required
            pattern="prj_[A-Za-z0-9_\-]{8,64}"
            className="font-mono"
          />
        </Field>
        <Field label="Verified backup ID" htmlFor={`${id}-backup`}>
          <Input
            id={`${id}-backup`}
            name="backupId"
            required
            maxLength={128}
            className="font-mono"
          />
        </Field>
        <Field label="Protected restore target" htmlFor={`${id}-target`}>
          <Input id={`${id}-target`} name="target" required maxLength={256} />
        </Field>
        <div className="flex items-center gap-2 self-end sm:h-9">
          <Checkbox id={`${id}-verified`} name="backupVerified" required />
          <Label htmlFor={`${id}-verified`} className="font-normal">
            Backup evidence reviewed and verified
          </Label>
        </div>
        <Field label="Impact preview" htmlFor={`${id}-impact`} className="sm:col-span-2">
          <Textarea id={`${id}-impact`} name="impactPreview" required maxLength={2048} />
        </Field>
        <Field label="Reason / case context" htmlFor={`${id}-reason`} className="sm:col-span-2">
          <Textarea id={`${id}-reason`} name="reason" required minLength={8} maxLength={1024} />
        </Field>
        <div className="sm:col-span-2">
          <Button type="submit" disabled={pending}>
            {pending ? "Requesting…" : "Review and request recovery"}
          </Button>
        </div>
      </form>
    </Disclosure>
  );
}

function ActivityPage({ permissions }: { readonly permissions: ReadonlySet<string> }) {
  const client = useOperatorClient();
  const id = useId();
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
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          setSubmitted(query.trim());
        }}
      >
        <Field
          label="Actor, action, target, tenant, case, request, or outcome"
          htmlFor={`${id}-query`}
          className="min-w-0 flex-1 basis-72"
        >
          <Input
            id={`${id}-query`}
            type="search"
            value={query}
            maxLength={200}
            onChange={(event) => setQuery(event.currentTarget.value)}
          />
        </Field>
        <Button type="submit">
          <Search aria-hidden="true" />
          Search
        </Button>
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
      <div className="grid gap-4 sm:grid-cols-3">
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
        <Alert role="status">
          <AlertDescription className="block text-foreground">{message}</AlertDescription>
        </Alert>
      )}
      <Card className="gap-0 overflow-hidden py-0">
        <Table>
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead scope="col" className="pl-4">
                Operator
              </TableHead>
              <TableHead scope="col">Developer identity</TableHead>
              <TableHead scope="col">Epoch</TableHead>
              <TableHead scope="col">Permissions</TableHead>
              <TableHead scope="col" className="pr-4 text-right">
                Action
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {value.entitlements.map((entitlement) => (
              <TableRow key={entitlement.developerIdentityId}>
                <TableCell className="pl-4">
                  <code className="font-mono text-xs">{entitlement.operatorId}</code>
                </TableCell>
                <TableCell>
                  <code className="font-mono text-xs">{entitlement.developerIdentityId}</code>
                </TableCell>
                <TableCell className="tabular-nums">{entitlement.operatorEpoch}</TableCell>
                <TableCell className="max-w-md whitespace-normal text-muted-foreground">
                  {entitlement.permissions.map(humanize).join(", ")}
                </TableCell>
                <TableCell className="pr-4 text-right">
                  {!canManage ? null : (
                    <Button
                      variant="outline"
                      size="sm"
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
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Authentication failures and throttling</CardTitle>
        </CardHeader>
        <CardContent>
          {value.attempts.length === 0 ? (
            <p className="m-0 text-sm text-muted-foreground">No retained attempt records.</p>
          ) : (
            <ul className="m-0 grid list-disc gap-1 pl-5 text-sm">
              {value.attempts.map((attempt) => (
                <li key={`${attempt.class}-${attempt.nextAllowedAt}-${attempt.count}`}>
                  {humanize(attempt.class)}: {attempt.count} attempt(s), next allowed{" "}
                  {formatRelative(attempt.nextAllowedAt)}
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>
    </>
  );
}

function EntitlementAdministration({ onChanged }: { readonly onChanged: () => void }) {
  const client = useOperatorClient();
  const id = useId();
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  return (
    <Disclosure summary="Administer operator entitlement">
      <p className="m-0 text-sm text-muted-foreground">
        The server plans and applies this change against the same digest. Long confirmation tokens
        are passed internally; you do not need to copy and paste one.
      </p>
      {message === null ? null : (
        <Alert role="status">
          <AlertDescription className="block text-foreground">{message}</AlertDescription>
        </Alert>
      )}
      <form
        className="grid gap-4 sm:grid-cols-2"
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
        <Field label="Change" htmlFor={`${id}-kind`}>
          <NativeSelect id={`${id}-kind`} name="kind">
            <option value="grant">Grant</option>
            <option value="replace">Replace</option>
            <option value="revoke">Revoke</option>
          </NativeSelect>
        </Field>
        <Field label="Developer email" htmlFor={`${id}-email`}>
          <Input id={`${id}-email`} name="targetEmail" type="email" required maxLength={320} />
        </Field>
        <fieldset className="m-0 grid gap-3 border-0 p-0 sm:col-span-2">
          <legend className="mb-2 p-0 text-sm font-medium leading-none">
            Permissions (ignored for revoke)
          </legend>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {OPERATOR_PERMISSIONS.map((permission) => (
              <div key={permission} className="flex items-center gap-2">
                <Checkbox id={`${id}-${permission}`} name={permission} />
                <Label htmlFor={`${id}-${permission}`} className="font-normal">
                  {humanize(permission)}
                </Label>
              </div>
            ))}
          </div>
        </fieldset>
        <Field
          label="Private reason / case context"
          htmlFor={`${id}-reason`}
          className="sm:col-span-2"
        >
          <Textarea
            id={`${id}-reason`}
            name="privateReason"
            required
            minLength={8}
            maxLength={1024}
          />
        </Field>
        <div className="sm:col-span-2">
          <Button type="submit" disabled={pending}>
            {pending ? "Planning and applying…" : "Review and apply"}
          </Button>
        </div>
      </form>
    </Disclosure>
  );
}

function ActivityTable({ value }: { readonly value: OperatorActivityPage }) {
  if (value.items.length === 0)
    return <WorkspaceEmptyState title="No activity matches these filters" />;
  return (
    <Card className="gap-0 overflow-hidden py-0">
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead scope="col" className="pl-4">
              Time
            </TableHead>
            <TableHead scope="col">Actor</TableHead>
            <TableHead scope="col">Action</TableHead>
            <TableHead scope="col">Target</TableHead>
            <TableHead scope="col">Outcome</TableHead>
            <TableHead scope="col" className="pr-4">
              Integrity
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {value.items.map((event) => (
            <TableRow key={event.id}>
              <TableCell className="pl-4 text-muted-foreground">
                {formatDate(event.timestamp)}
              </TableCell>
              <TableCell>
                <code className="font-mono text-xs">{event.actorId}</code>
              </TableCell>
              <TableCell>{humanize(event.action)}</TableCell>
              <TableCell>
                <code className="font-mono text-xs">{event.target}</code>
              </TableCell>
              <TableCell>{event.outcome}</TableCell>
              <TableCell className="pr-4">
                <code className="font-mono text-xs text-muted-foreground">
                  {event.integrity.slice(0, 12)}…
                </code>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Card>
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
    <div className="flex flex-wrap items-center gap-3">
      <Button
        variant="outline"
        size="sm"
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
      </Button>
      {message === null ? null : (
        <span role="status" className="text-sm text-muted-foreground">
          {message}
        </span>
      )}
    </div>
  );
}

function WorkspacePage({
  title,
  eyebrow,
  subtitle,
  actions,
  children,
}: {
  readonly title: string;
  readonly eyebrow: ReactNode;
  readonly subtitle?: string;
  readonly actions?: ReactNode;
  readonly children: ReactNode;
}) {
  return (
    <section className="grid gap-6" aria-labelledby="operator-page-title">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <Eyebrow>{eyebrow}</Eyebrow>
          <h1 id="operator-page-title" className="text-2xl">
            {title}
          </h1>
          {subtitle === undefined ? null : (
            <p className="m-0 max-w-2xl text-sm text-muted-foreground">{subtitle}</p>
          )}
        </div>
        {actions}
      </div>
      {children}
    </section>
  );
}

function SectionGrid({ sections }: { readonly sections: readonly OperatorReadSection[] }) {
  if (sections.length === 0)
    return <WorkspaceEmptyState title="No provider sections are configured" />;
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      {sections.map((section) => (
        <Card className="gap-4" key={section.id}>
          <CardHeader>
            <Eyebrow>{section.provider}</Eyebrow>
            <CardTitle>{humanize(section.id)}</CardTitle>
            <CardAction>
              <FreshnessBadge value={section.freshness} />
            </CardAction>
          </CardHeader>
          <CardContent className="grid gap-3 text-sm">
            {section.message === null ? null : <p className="m-0">{section.message}</p>}
            {Object.keys(section.metrics).length === 0 ? null : (
              <dl className="m-0 grid gap-1.5">
                {Object.entries(section.metrics).map(([name, value]) => (
                  <div
                    key={name}
                    className="flex items-baseline justify-between gap-3 border-b border-dashed pb-1.5 last:border-b-0 last:pb-0"
                  >
                    <dt className="text-muted-foreground">{humanize(name)}</dt>
                    <dd className="m-0 text-right font-medium wrap-anywhere tabular-nums">
                      {metricValue(value)}
                    </dd>
                  </div>
                ))}
              </dl>
            )}
            {section.links.length === 0 ? null : (
              <ul className="m-0 grid list-none gap-1 p-0">
                {section.links.map((link) => (
                  <li key={link.url}>
                    <a
                      className="inline-flex items-center gap-1"
                      href={link.url}
                      target="_blank"
                      rel="noreferrer"
                    >
                      {link.label}
                      <ExternalLink aria-hidden="true" className="size-3.5" />
                    </a>
                  </li>
                ))}
              </ul>
            )}
            <small className="text-xs text-muted-foreground">
              Observed{" "}
              {section.observedAt === null ? "unknown" : formatRelative(section.observedAt)}
            </small>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}

/** How fresh a provider's answer is: current, stale, or absent. */
function FreshnessBadge({ value }: { readonly value: string }) {
  const variant =
    value === "current"
      ? "positive"
      : value === "unavailable"
        ? "destructive"
        : value === "stale"
          ? "warning"
          : "secondary";
  return <Badge variant={variant}>{humanize(value)}</Badge>;
}

/** A project or environment lifecycle word. */
function LifecycleBadge({ value }: { readonly value: string }) {
  const variant =
    value === "active"
      ? "positive"
      : value === "suspended" || value === "deleted" || value === "failed"
        ? "destructive"
        : value === "provisioning" || value === "pending" || value === "deleting"
          ? "warning"
          : "secondary";
  return <Badge variant={variant}>{humanize(value)}</Badge>;
}

function SeverityBadge({ value }: { readonly value: string }) {
  const variant =
    value === "critical"
      ? "destructive"
      : value === "high"
        ? "warning"
        : value === "medium"
          ? "secondary"
          : "outline";
  return <Badge variant={variant}>{value}</Badge>;
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
    <Card className="gap-2 py-4">
      <CardContent className="grid gap-1">
        <Eyebrow>{label}</Eyebrow>
        <div className="flex items-center gap-2">
          <strong className="text-2xl font-semibold tracking-tight tabular-nums">{value}</strong>
          <span
            className={cn(
              "size-2 shrink-0 rounded-full",
              tone === "current"
                ? "bg-positive"
                : tone === "unavailable"
                  ? "bg-destructive"
                  : tone === "stale"
                    ? "bg-warning"
                    : "bg-muted-foreground",
            )}
            aria-hidden="true"
          />
        </div>
      </CardContent>
    </Card>
  );
}

/** Nothing to list; the title keeps its place in the page's outline. */
function WorkspaceEmptyState({ title }: { readonly title: string }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed px-6 py-10 text-center">
      <div className="mb-1 flex size-10 items-center justify-center rounded-full bg-muted text-muted-foreground">
        <Inbox aria-hidden="true" className="size-5" />
      </div>
      <h2 className="text-base">{title}</h2>
      <p className="m-0 max-w-sm text-sm text-muted-foreground">
        Adjust filters or refresh after the underlying state changes.
      </p>
    </div>
  );
}

function LoadingState({ label }: { readonly label: string }) {
  return (
    <p
      className="m-0 flex items-center gap-2 rounded-xl border bg-card px-5 py-4 text-sm text-muted-foreground"
      aria-live="polite"
      aria-busy="true"
    >
      <LoaderCircle aria-hidden="true" className="size-4 shrink-0 animate-spin" />
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
    <Button variant="outline" size="sm" disabled={pending} onClick={reload}>
      <RefreshCw aria-hidden="true" className={cn(pending && "animate-spin")} />
      {pending ? "Refreshing…" : "Refresh"}
    </Button>
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
    <nav
      className="flex flex-wrap items-center justify-between gap-2"
      aria-label="Cursor pagination"
    >
      <Button variant="outline" size="sm" disabled={!canPrevious} onClick={onPrevious}>
        <ChevronLeft aria-hidden="true" />
        Previous page
      </Button>
      <span className="text-xs text-muted-foreground">Up to 25 records per page</span>
      <Button variant="outline" size="sm" disabled={!canNext} onClick={onNext}>
        Next page
        <ChevronRight aria-hidden="true" />
      </Button>
    </nav>
  );
}

/** A workflow folded away until it is wanted: a card whose body is a `details`. */
function Disclosure({
  summary,
  children,
}: {
  readonly summary: string;
  readonly children: ReactNode;
}) {
  return (
    <Card as="div" className="gap-0 py-0">
      <details className="group m-0">
        <summary className="m-0 flex cursor-pointer list-none items-center gap-2 px-5 py-4 text-base font-semibold tracking-tight [&::-webkit-details-marker]:hidden">
          <ChevronRight
            aria-hidden="true"
            className="size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-90"
          />
          {summary}
        </summary>
        <div className="grid gap-4 border-t px-5 py-5">{children}</div>
      </details>
    </Card>
  );
}

function Definition({ term, children }: { readonly term: string; readonly children: ReactNode }) {
  return (
    <div className="grid gap-1">
      <dt className="text-xs font-semibold tracking-wider text-muted-foreground uppercase">
        {term}
      </dt>
      <dd className="m-0 text-sm wrap-anywhere">{children}</dd>
    </div>
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
