import type {
  Collection,
  ConnectionCheck,
  ConnectMetadata,
  DeveloperBackup,
  DeveloperRestore,
  Environment,
  Project,
  SyncSummary,
  WorkspaceDestination,
} from "@mako-cloud/management-sdk";
import { createMakoRxdbConnectTemplateV1 } from "@mako-cloud/rxdb";
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
  cn,
  EmptyState,
  Eyebrow,
  Field,
  Input,
  Label,
  NativeSelect,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
} from "@mako-cloud/ui";
import {
  Activity,
  BookOpen,
  Box,
  CalendarClock,
  ChevronRight,
  Copy,
  Database,
  DatabaseBackup,
  FolderOpen,
  Gauge,
  Globe,
  HardDrive,
  KeyRound,
  LayoutDashboard,
  type LucideIcon,
  Mail,
  Plug,
  Radar,
  RefreshCw,
  ScrollText,
  Settings,
  ShieldCheck,
  SquareFunction,
  Table2,
  TriangleAlert,
  Users,
  Webhook,
} from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { AllowedOriginsSection } from "./allowed-origins.js";
import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { DataExplorer } from "./data-explorer.js";
import { DatabaseOverview } from "./database-overview.js";
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
        key={`${projectId}:${environmentId}`}
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
      <CollectionPolicies projectId={projectId} environmentId={environmentId} navigate={navigate} />
    );
  } else if (section === "settings") {
    content = <EnvironmentSettings projectId={projectId} environmentId={environmentId} />;
  } else {
    content = (
      <DatabaseOverview projectId={projectId} environmentId={environmentId} navigate={navigate} />
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

/** One icon per destination the sidebar can list; an unknown id gets a plain box. */
const DESTINATION_ICONS: Readonly<Record<string, LucideIcon>> = {
  overview: LayoutDashboard,
  data: Database,
  collections: Table2,
  sync: RefreshCw,
  users: Users,
  policies: ShieldCheck,
  functions: SquareFunction,
  observability: Radar,
  backups: DatabaseBackup,
  connect: Plug,
  settings: Settings,
  credentials: KeyRound,
  storage: HardDrive,
  webhooks: Webhook,
  "auth-providers": KeyRound,
  "email-templates": Mail,
  "api-docs": BookOpen,
  logs: ScrollText,
  usage: Gauge,
  activity: Activity,
  schedules: CalendarClock,
  domains: Globe,
};

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
  const [navigationFailure, setNavigationFailure] = useState<ConsoleApiFailure | null>(null);
  const [navigationLoaded, setNavigationLoaded] = useState(false);
  useEffect(() => {
    let active = true;
    void Promise.all([client.getProject(projectId), client.listEnvironments(projectId)]).then(
      ([nextProject, nextEnvironments]) => {
        if (!active) return;
        setProject(nextProject);
        setEnvironments(nextEnvironments);
        setFailure(null);
      },
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    void client.getWorkspaceNavigation(projectId, environmentId).then(
      (value) => {
        if (active) {
          setDestinations(value);
          setNavigationFailure(null);
          setNavigationLoaded(true);
        }
      },
      (error: unknown) => {
        if (!active) return;
        setNavigationFailure(toConsoleApiFailure(error));
        setNavigationLoaded(true);
      },
    );
    return () => {
      active = false;
    };
  }, [client, environmentId, projectId]);
  const allDestinations = [
    ...destinations.filter((destination) => destination.permitted),
    ...(!destinations.some(
      (destination) => ["overview", "data"].includes(destination.id) && destination.permitted,
    )
      ? []
      : consoleDestinations(projectId, environmentId).filter(
          (destination) => !destinations.some((item) => item.id === destination.id),
        )),
    ...(destinations.some(
      (destination) => destination.id === "settings" && destination.permitted,
    ) && !destinations.some((destination) => destination.id === "credentials")
      ? [
          {
            id: "credentials",
            label: "API keys",
            path: `/projects/${projectId}/environments/${environmentId}/credentials`,
            permitted: true,
          },
        ]
      : []),
  ];
  const groups = [
    { label: "Database", ids: ["overview", "data", "collections", "policies", "sync", "backups"] },
    {
      label: "Application",
      ids: ["users", "auth-providers", "email-templates", "storage", "functions", "webhooks"],
    },
    { label: "Observe", ids: ["observability", "logs", "usage", "activity"] },
    { label: "Configure", ids: ["connect", "credentials", "api-docs", "settings"] },
  ];
  const currentEnvironment = environments.find((environment) => environment.id === environmentId);
  // Until the project answers, its name is unknown: a placeholder holds its
  // place. The raw id stood in before, and an id has nowhere to wrap, so it
  // widened the sidebar past its edge and the environment picker with it.
  const projectLoading = project === null && failure === null;
  const projectLabel = project?.name ?? projectId;
  const environmentLabel = currentEnvironment?.name ?? environmentId;
  return (
    <div className="grid min-h-[calc(100vh-3.5rem)] grid-cols-1 md:grid-cols-[16rem_minmax(0,1fr)]">
      <aside
        // Full height below the top bar, so the navigation's ground reaches the
        // bottom of the window rather than stopping under its last destination.
        className="flex min-w-0 flex-col gap-5 border-b bg-sidebar px-3 py-5 text-sidebar-foreground md:sticky md:top-0 md:h-[calc(100vh-3.5rem)] md:self-start md:overflow-y-auto md:border-r md:border-b-0"
        aria-label="Environment navigation"
      >
        <div className="grid gap-3 px-3">
          <a
            href="/"
            className="text-xs text-muted-foreground"
            onClick={(event) => {
              if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
              event.preventDefault();
              navigate("/");
            }}
          >
            ← All projects
          </a>
          <Eyebrow>Database workspace</Eyebrow>
          <Button
            variant="ghost"
            aria-label={projectLoading ? "Project" : undefined}
            className="h-auto min-w-0 items-start justify-start gap-2 whitespace-normal px-0 py-0 text-left text-base font-semibold hover:bg-transparent hover:underline"
            onClick={() => navigate(`/projects/${projectId}`)}
          >
            <FolderOpen aria-hidden="true" className="mt-1 size-4 shrink-0 text-muted-foreground" />
            {projectLoading ? (
              <Skeleton className="h-5 w-36" />
            ) : (
              <span className="line-clamp-2 min-w-0 [overflow-wrap:anywhere]" title={projectLabel}>
                {projectLabel}
              </span>
            )}
          </Button>
          <div className="grid gap-1">
            <Label htmlFor="environment-switcher" className="text-xs text-muted-foreground">
              Environment
            </Label>
            {projectLoading ? (
              <Skeleton className="h-8 w-full" />
            ) : (
              <NativeSelect
                id="environment-switcher"
                size="sm"
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
              </NativeSelect>
            )}
          </div>
        </div>
        <nav aria-label="Environment destinations" className="min-w-0">
          <div className="grid gap-1 px-3 md:hidden">
            <Label htmlFor="workspace-destination" className="text-xs text-muted-foreground">
              Go to
            </Label>
            <NativeSelect
              id="workspace-destination"
              value={section}
              onChange={(event) => {
                const destination = allDestinations.find(
                  (item) => item.id === event.currentTarget.value,
                );
                if (destination) navigate(destination.path);
              }}
            >
              {groups.map((group) => (
                <optgroup key={group.label} label={group.label}>
                  {allDestinations
                    .filter((item) => group.ids.includes(item.id))
                    .map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.label}
                      </option>
                    ))}
                </optgroup>
              ))}
            </NativeSelect>
          </div>
          <div className="hidden md:grid md:gap-4">
            {groups.map((group) => (
              <div key={group.label} className="min-w-40 md:min-w-0">
                <Eyebrow className="mb-1 px-3 text-[10px]">{group.label}</Eyebrow>
                <ul className="m-0 grid list-none gap-0.5 p-0">
                  {navigationLoaded
                    ? null
                    : group.ids.map((id) => (
                        <li
                          key={id}
                          aria-hidden="true"
                          className="flex items-center gap-3 border border-transparent px-3 py-1.5"
                        >
                          <Skeleton className="size-4 shrink-0 rounded-sm" />
                          <Skeleton className="my-0.5 h-4 w-24" />
                        </li>
                      ))}
                  {allDestinations
                    .filter((destination) => group.ids.includes(destination.id))
                    .map((destination) => {
                      const Icon = DESTINATION_ICONS[destination.id] ?? Box;
                      return (
                        <li key={destination.id}>
                          <a
                            className={cn(
                              "flex items-center gap-3 rounded-md border border-transparent px-3 py-1.5 text-sm text-muted-foreground no-underline hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:no-underline",
                              section === destination.id &&
                                "border-primary/15 bg-primary/10 font-medium text-primary",
                            )}
                            aria-current={section === destination.id ? "page" : undefined}
                            href={destination.path}
                            onClick={(event) => {
                              if (
                                event.ctrlKey ||
                                event.metaKey ||
                                event.shiftKey ||
                                event.altKey ||
                                event.button !== 0
                              )
                                return;
                              event.preventDefault();
                              navigate(destination.path);
                            }}
                          >
                            <Icon aria-hidden="true" className="size-4 shrink-0" />
                            {destination.label}
                          </a>
                        </li>
                      );
                    })}
                </ul>
              </div>
            ))}
          </div>
        </nav>
      </aside>
      <div className="min-w-0 px-6 py-6">
        <nav
          className="mb-6 flex flex-wrap items-center gap-1.5 text-sm text-muted-foreground"
          aria-label="Breadcrumb"
        >
          <a
            href="/"
            onClick={(event) => {
              event.preventDefault();
              navigate("/");
            }}
          >
            Projects
          </a>
          <span aria-hidden="true" className="flex text-muted-foreground/60">
            <ChevronRight className="size-3.5" />
          </span>
          <a
            href={`/projects/${projectId}`}
            onClick={(event) => {
              event.preventDefault();
              navigate(`/projects/${projectId}`);
            }}
          >
            {projectLoading ? (
              <Skeleton className="inline-block h-4 w-28 align-middle" />
            ) : (
              <span className="block max-w-64 truncate" title={projectLabel}>
                {projectLabel}
              </span>
            )}
          </a>
          <span aria-hidden="true" className="flex text-muted-foreground/60">
            <ChevronRight className="size-3.5" />
          </span>
          <strong className="font-medium text-foreground">
            {projectLoading ? (
              <Skeleton className="inline-block h-4 w-24 align-middle" />
            ) : (
              <span className="block max-w-64 truncate" title={environmentLabel}>
                {environmentLabel}
              </span>
            )}
          </strong>
        </nav>
        <ApiFailureNotice failure={failure} />
        <ApiFailureNotice failure={navigationFailure} />
        {children}
      </div>
    </div>
  );
}

/** A page's title line: eyebrow, h1, a muted sentence, and room for an action on the right. */
function PageHeader({
  eyebrow,
  title,
  description,
  action,
}: {
  readonly eyebrow: string;
  readonly title: string;
  readonly description?: ReactNode;
  readonly action?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-4">
      <div className="grid gap-1">
        <Eyebrow>{eyebrow}</Eyebrow>
        <h1 className="text-2xl">{title}</h1>
        {description === undefined ? null : (
          <p className="m-0 max-w-2xl text-sm text-muted-foreground">{description}</p>
        )}
      </div>
      {action}
    </div>
  );
}

/**
 * A warning the reader should notice but that is not a live region: the page
 * may already own its one `status`, and these notes are part of the page, not
 * news about it.
 */
function WarningNote({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <Alert variant="warning" role="note" className={className}>
      <TriangleAlert aria-hidden="true" />
      <AlertDescription className="block">{children}</AlertDescription>
    </Alert>
  );
}

/** One fact in a grid of facts: a small label over its value. */
function Definition({ term, children }: { readonly term: string; readonly children: ReactNode }) {
  return (
    <div className="grid gap-1 rounded-lg border bg-card px-3 py-2.5">
      <dt className="text-xs font-medium text-muted-foreground">{term}</dt>
      <dd className="m-0 text-sm break-all">{children}</dd>
    </div>
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
    <section className="grid gap-6">
      <PageHeader
        eyebrow="API & Connect"
        title="Connect an RxDB application"
        description="Only the public project key belongs in browser or mobile code. Never ship a service credential."
      />
      <ApiFailureNotice failure={failure} />
      {metadata === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading public connection metadata…</p>
      ) : (
        <>
          <dl className="m-0 grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <Definition term="Public endpoint">
              <code className="font-mono">{metadata.publicEndpoint}</code>
            </Definition>
            <Definition term="Public key ID">
              <code className="font-mono">{metadata.publicKeyId}</code>
            </Definition>
            <Definition term="Supported client">
              <code className="font-mono">{metadata.rxdbClientRange}</code>
            </Definition>
            <Definition term="Template">v{metadata.templateVersion}</Definition>
          </dl>
          {metadata.publicKey === "" ? (
            <WarningNote>
              <p>
                No recoverable public key is available. Issue or rotate a public project credential
                on the Credentials page, then paste the one-time value into your application secret
                store.
              </p>
            </WarningNote>
          ) : (
            <Field label="Public project key" htmlFor="connect-public-key">
              <Input
                id="connect-public-key"
                readOnly
                value={metadata.publicKey}
                className="font-mono text-xs"
              />
            </Field>
          )}
          <Field label="Collection" htmlFor="connect-collection" className="max-w-md">
            <NativeSelect
              id="connect-collection"
              value={collectionId}
              onChange={(event) => setCollectionId(event.currentTarget.value)}
            >
              {metadata.collections.map((collection) => (
                <option key={collection.collectionId} value={collection.collectionId}>
                  {collection.collectionId} · schema v{collection.activeSchemaVersion}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <div className="relative overflow-hidden rounded-lg border bg-muted/40">
            <Button
              variant="outline"
              size="sm"
              className="absolute top-2 right-2"
              onClick={() => void navigator.clipboard.writeText(snippet)}
            >
              <Copy aria-hidden="true" />
              Copy
            </Button>
            <pre className="m-0 overflow-x-auto p-4 pr-24 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words">
              {snippet}
            </pre>
          </div>
          <Card>
            <CardHeader>
              <CardTitle>Connection check</CardTitle>
              <CardDescription>
                Checks DNS, TLS, public routing, readiness, key metadata, schema compatibility, and
                replication routes without reading documents or creating a user session.
              </CardDescription>
              <CardAction>
                <Button onClick={() => void runCheck()}>Run check</Button>
              </CardAction>
            </CardHeader>
            {check !== null ? (
              <CardContent>
                <ol className="m-0 grid list-none gap-3 p-0">
                  {check.steps.map((step) => (
                    <li key={step.id} className="flex items-start gap-3">
                      <StatusBadge state={step.state} />
                      <span className="grid gap-0.5 text-sm">
                        <strong className="font-medium">{humanize(step.id)}</strong>
                        {step.remediationCode !== null && step.remediationCode !== undefined ? (
                          <small className="text-xs text-muted-foreground">
                            {humanize(step.remediationCode)} ·{" "}
                            {step.retryable ? "retryable" : "configuration change required"}
                          </small>
                        ) : null}
                      </span>
                    </li>
                  ))}
                </ol>
              </CardContent>
            ) : null}
          </Card>
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
    <section className="grid gap-6">
      <PageHeader eyebrow="RxDB sync" title="Replication diagnostics" />
      <ApiFailureNotice failure={failure} />
      <form
        className="grid gap-3 sm:grid-cols-[minmax(12rem,1fr)_minmax(12rem,1fr)_auto] sm:items-end"
        onSubmit={(event) => void query(event)}
      >
        <Field label="Collection" htmlFor="sync-collection">
          <NativeSelect id="sync-collection" name="collectionId">
            <option value="">All collections</option>
            {collections.map((collection) => (
              <option key={collection.id} value={collection.id}>
                {collection.id}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label="Time window" htmlFor="sync-hours">
          <NativeSelect id="sync-hours" name="hours" defaultValue="1">
            <option value="1">Last hour</option>
            <option value="6">Last 6 hours</option>
            <option value="24">Last 24 hours</option>
          </NativeSelect>
        </Field>
        <Button type="submit">Apply</Button>
      </form>
      {summary === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading sync summary…</p>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-6">
            {metrics.map(([label, value]) => (
              <Card as="article" key={label} className="gap-1 px-4 py-3">
                <strong className="text-2xl font-semibold tabular-nums">{value ?? 0}</strong>
                <span className="text-xs text-muted-foreground">{label}</span>
              </Card>
            ))}
          </div>
          <Card>
            <CardHeader>
              <CardTitle>Client compatibility classes</CardTitle>
            </CardHeader>
            <CardContent className="grid gap-3 text-sm">
              {Object.keys(summary.clientVersionClasses ?? {}).length === 0 ? (
                <p className="m-0">No bounded client-version observations in this window.</p>
              ) : (
                <ul className="m-0 grid gap-1 pl-5">
                  {Object.entries(summary.clientVersionClasses ?? {}).map(([label, count]) => (
                    <li key={label}>
                      {humanize(label)}: {count}
                    </li>
                  ))}
                </ul>
              )}
              <p className="m-0 text-xs text-muted-foreground">
                Observed {formatTime(summary.observedAtUnixSeconds)} · retained since{" "}
                {formatTime(summary.retainedSinceUnixSeconds)}. Counts never contain raw user,
                device, session, IP, token, or document identifiers.
              </p>
            </CardContent>
          </Card>
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
    <aside className="flex items-start gap-3 rounded-lg border border-warning/40 bg-warning/10 px-4 py-3 text-sm">
      <TriangleAlert aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
      <div className="grid gap-1">
        <strong className="font-medium">Recommended action</strong>
        <ul className="m-0 grid gap-1 pl-5 text-foreground/85">
          {issues.map((issue) => (
            <li key={issue.text}>{issue.text}</li>
          ))}
        </ul>
      </div>
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
    <section className="grid gap-6">
      <PageHeader
        eyebrow="Backup and recovery"
        title="Verified recovery points"
        description="Developer recovery always creates a new isolated environment. It cannot overwrite or promote an environment."
      />
      <ApiFailureNotice failure={failure} />
      {backups.length === 0 ? (
        <EmptyState
          icon={<DatabaseBackup aria-hidden="true" />}
          title="No tenant-verified recovery point is available."
        />
      ) : (
        <Card className="gap-0 overflow-hidden py-0">
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col" className="pl-4">
                  Recovery point
                </TableHead>
                <TableHead scope="col">Verification</TableHead>
                <TableHead scope="col">Retention</TableHead>
                <TableHead scope="col" className="pr-4">
                  Restore drill / objective
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {backups.map((backup) => (
                <TableRow key={backup.backupId}>
                  <TableCell className="pl-4">
                    <code className="font-mono text-xs">{backup.backupId}</code>
                    <span className="block text-xs text-muted-foreground">
                      {formatTime(backup.recoveryPointUnixSeconds)}
                    </span>
                  </TableCell>
                  <TableCell>{formatTime(backup.verifiedAtUnixSeconds)}</TableCell>
                  <TableCell>{formatTime(backup.retainedUntilUnixSeconds)}</TableCell>
                  <TableCell className="pr-4">
                    {backup.lastRestoreDrillUnixSeconds === null ||
                    backup.lastRestoreDrillUnixSeconds === undefined
                      ? "No recorded drill"
                      : formatTime(backup.lastRestoreDrillUnixSeconds)}
                    <span className="block text-xs text-muted-foreground">
                      {backup.recoveryObjectiveStatus}
                    </span>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </Card>
      )}
      {restoreEnabled ? (
        <Card>
          <CardHeader>
            <CardTitle>Request isolated recovery environment</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-5">
            <WarningNote>
              <p>
                The restored environment stays inaccessible until tenant isolation, storage,
                services, and recovery validation all pass. Overwrite and promotion are prohibited.
              </p>
            </WarningNote>
            <form className="grid gap-4" onSubmit={(event) => void requestRestore(event)}>
              <Field label="Verified backup" htmlFor="restore-backup">
                <NativeSelect id="restore-backup" name="backupId" required>
                  {backups.map((backup) => (
                    <option key={backup.backupId} value={backup.backupId}>
                      {backup.backupId} · {formatTime(backup.recoveryPointUnixSeconds)}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Field label="New environment name" htmlFor="restore-target-name">
                <Input
                  id="restore-target-name"
                  name="targetEnvironmentName"
                  required
                  maxLength={64}
                />
              </Field>
              <Field label="Reason" htmlFor="restore-reason">
                <Textarea id="restore-reason" name="reason" required maxLength={500} />
              </Field>
              <Field label="Confirm your developer password" htmlFor="restore-password">
                <Input
                  id="restore-password"
                  name="password"
                  type="password"
                  required
                  autoComplete="current-password"
                />
              </Field>
              <div>
                <Button type="submit" disabled={backups.length === 0}>
                  Request recovery
                </Button>
              </div>
            </form>
          </CardContent>
        </Card>
      ) : (
        <DisabledWorkspaceFeature name="Isolated recovery requests" />
      )}
      <Card>
        <CardHeader>
          <CardTitle>Recovery requests</CardTitle>
        </CardHeader>
        <CardContent>
          {restores.length === 0 ? (
            <p className="m-0 text-sm text-muted-foreground">No recovery requests.</p>
          ) : (
            <div className="grid gap-2">
              {restores.map((restore) => (
                <div
                  className="flex items-center justify-between gap-4 rounded-lg border px-3 py-2"
                  key={restore.requestId}
                >
                  <span className="grid gap-0.5 text-sm">
                    <strong className="font-medium">{restore.target.environmentId}</strong>
                    <small className="text-xs text-muted-foreground">
                      {restore.state} · updated {formatTime(restore.updatedAtUnixSeconds)}
                    </small>
                  </span>
                  <StatusBadge state={restore.accessible ? "ready" : restore.state} />
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
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
    <section className="grid gap-6">
      <PageHeader eyebrow="Settings" title="Environment settings" />
      <ApiFailureNotice failure={failure} />
      {environment === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading…</p>
      ) : (
        <dl className="m-0 grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <Definition term="Name">{environment.name}</Definition>
          <Definition term="ID">
            <code className="font-mono">{environment.id}</code>
          </Definition>
          <Definition term="Lifecycle">
            <StatusBadge state={environment.state} />
          </Definition>
          <Definition term="Updated">{new Date(environment.updatedAt).toLocaleString()}</Definition>
        </dl>
      )}
      <Alert role="note">
        <AlertDescription className="block">
          Credentials, schema, function, and destructive lifecycle controls remain on their existing
          dedicated pages while the workspace rollout is reversible.
        </AlertDescription>
      </Alert>
      <AllowedOriginsSection projectId={projectId} environmentId={environmentId} />
    </section>
  );
}

function CollectionPolicies({
  projectId,
  environmentId,
  navigate,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly navigate: (path: string) => void;
}) {
  const client = useManagementClient();
  const [collections, setCollections] = useState<Collection[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  useEffect(() => {
    let active = true;
    void client.listCollections(projectId, environmentId).then(
      (value) => active && setCollections(value),
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
    };
  }, [client, projectId, environmentId]);
  return (
    <section className="grid gap-6">
      <PageHeader
        eyebrow="Database access"
        title="Collection policies"
        description="Define which documents an application user can read or write. Validate and preview a policy before activating it."
      />
      <ApiFailureNotice failure={failure} />
      {collections === null ? (
        failure === null ? (
          <p className="text-sm text-muted-foreground">Loading collections…</p>
        ) : null
      ) : collections.length === 0 ? (
        <EmptyState
          title="Create a collection first"
          description="Policies belong to a collection and validate against its schema."
          action={
            <Button
              onClick={() =>
                navigate(`/projects/${projectId}/environments/${environmentId}/collections`)
              }
            >
              Create collection
            </Button>
          }
        />
      ) : (
        <Card>
          <CardContent className="pt-1">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Collection</TableHead>
                  <TableHead>Schema</TableHead>
                  <TableHead>State</TableHead>
                  <TableHead>Policy</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {collections.map((collection) => (
                  <TableRow key={collection.id}>
                    <TableCell className="font-mono">{collection.id}</TableCell>
                    <TableCell>v{collection.schemaVersion}</TableCell>
                    <TableCell>{collection.state}</TableCell>
                    <TableCell>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() =>
                          navigate(
                            `/projects/${projectId}/environments/${environmentId}/collections/${collection.id}/policies`,
                          )
                        }
                      >
                        <ShieldCheck aria-hidden="true" />
                        Manage policy
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      )}
    </section>
  );
}

function DisabledWorkspaceFeature({ name }: { readonly name: string }) {
  return (
    <Card className="max-w-2xl">
      <CardHeader>
        <Eyebrow>Staged rollout</Eyebrow>
        <CardTitle as="h1" className="text-xl">
          {name}
        </CardTitle>
        <CardDescription>
          This capability is disabled by its independent deployment gate. Existing project tools
          remain available.
        </CardDescription>
      </CardHeader>
    </Card>
  );
}
function StatusBadge({ state }: { readonly state: string }) {
  const tone =
    state === "current" || state === "passed" || state === "ready" || state === "active"
      ? "success"
      : state === "unavailable" || state === "failed"
        ? "error"
        : "warning";
  return (
    <Badge
      variant="outline"
      className={cn(
        "status-badge",
        tone,
        tone === "success" && "border-positive/40 bg-positive/10 text-positive",
        tone === "error" && "border-destructive/40 bg-destructive/10 text-destructive",
        tone === "warning" && "border-warning/50 bg-warning/15 text-foreground",
      )}
    >
      {humanize(state)}
    </Badge>
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
