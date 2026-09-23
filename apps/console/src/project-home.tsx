// Project home: the project-level shell with overview, usage, activity, and
// settings. It is a read-only aggregation of endpoints the console already
// calls — project, environments, owner, and for the selected environment the
// workspace summary, connect metadata, usage, health, and audit events. Every
// summary loads on its own and states when it was observed, so one failing
// source marks only its own panel.

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
import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
  cn,
  EmptyState,
  Eyebrow,
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
import {
  Activity,
  Copy,
  Database,
  Gauge,
  Globe,
  Layers,
  LayoutDashboard,
  type LucideIcon,
  Plus,
  RefreshCw,
  Settings,
} from "lucide-react";
import {
  type FormEvent,
  type MouseEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

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

const PROJECT_DESTINATIONS: readonly {
  readonly id: ProjectSection;
  readonly label: string;
  readonly icon: LucideIcon;
}[] = [
  { id: "overview", label: "Overview", icon: LayoutDashboard },
  { id: "usage", label: "Usage", icon: Gauge },
  { id: "activity", label: "Activity", icon: Activity },
  { id: "domains", label: "Domains", icon: Globe },
  { id: "settings", label: "Settings", icon: Settings },
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

/** A quiet secondary line: an identifier's caption, an observation time. */
const OBSERVED = "block text-xs text-muted-foreground";

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
    content = <p className="m-0 text-sm text-muted-foreground">Loading project…</p>;
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
    <div className="grid min-h-full lg:grid-cols-[16rem_minmax(0,1fr)]">
      <aside
        className="flex flex-col gap-6 border-b bg-sidebar p-4 text-sidebar-foreground lg:sticky lg:top-0 lg:max-h-screen lg:overflow-y-auto lg:border-r lg:border-b-0"
        aria-label="Project navigation"
      >
        <div className="grid gap-2">
          <Eyebrow>Project</Eyebrow>
          <ConsoleLink className="w-fit text-sm font-medium" path={ownerPath} navigate={navigate}>
            {ownerLabel}
          </ConsoleLink>
          <Button
            variant="ghost"
            className="-mx-2 h-auto justify-start px-2 py-1 text-left text-base font-semibold whitespace-normal"
            onClick={() => navigate(`/projects/${projectId}`)}
          >
            {project?.name ?? projectId}
          </Button>
        </div>
        <div className="grid gap-2">
          <Eyebrow id="project-environments-label">Environments</Eyebrow>
          {environments === null ? (
            <small className="text-xs text-muted-foreground">Loading…</small>
          ) : environments.length === 0 ? (
            <small className="text-xs text-muted-foreground">No environments yet</small>
          ) : (
            <ul
              aria-labelledby="project-environments-label"
              className="m-0 grid list-none gap-0.5 p-0"
            >
              {environments.map((environment) => (
                <li key={environment.id}>
                  <ConsoleLink
                    className="flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-sm text-foreground no-underline transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:no-underline"
                    path={`/projects/${projectId}/environments/${environment.id}/overview`}
                    navigate={navigate}
                  >
                    <span className="truncate">{environment.name}</span>
                    <LifecycleBadge state={environment.state} />
                  </ConsoleLink>
                </li>
              ))}
            </ul>
          )}
        </div>
        {selectedEnvironment === null ? null : (
          <nav aria-label="Database tools" className="grid gap-1">
            <Eyebrow className="mb-1 px-3">Database · {selectedEnvironment.name}</Eyebrow>
            {[
              ["data", "Browse data"],
              ["collections", "Collections & schema"],
              ["connect", "Connect application"],
              ["backups", "Backups"],
            ].map(([destination, label]) => (
              <ConsoleLink
                key={destination}
                className="flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-foreground no-underline hover:bg-sidebar-accent"
                path={`/projects/${projectId}/environments/${selectedEnvironment.id}/${destination}`}
                navigate={navigate}
              >
                <Database className="size-4 text-muted-foreground" aria-hidden="true" />
                {label}
              </ConsoleLink>
            ))}
          </nav>
        )}
        <nav aria-label="Project destinations">
          <ul className="m-0 grid list-none gap-0.5 p-0">
            {PROJECT_DESTINATIONS.map((destination) => {
              const Icon = destination.icon;
              const active = section === destination.id;
              return (
                <li key={destination.id}>
                  <ConsoleLink
                    className={cn(
                      "flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-muted-foreground no-underline transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:no-underline",
                      active && "bg-sidebar-accent text-sidebar-foreground",
                    )}
                    current={active}
                    path={
                      destination.id === "overview"
                        ? `/projects/${projectId}`
                        : `/projects/${projectId}/${destination.id}`
                    }
                    navigate={navigate}
                  >
                    <Icon aria-hidden="true" className="size-4 shrink-0" />
                    {destination.label}
                  </ConsoleLink>
                </li>
              );
            })}
          </ul>
        </nav>
      </aside>
      <div className="grid min-w-0 content-start gap-6 p-6 lg:p-8">
        <div className="grid gap-4">
          <nav
            className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
            aria-label="Breadcrumb"
          >
            <ConsoleLink path="/" navigate={navigate}>
              Home
            </ConsoleLink>
            <span aria-hidden="true">/</span>
            <ConsoleLink path={ownerPath} navigate={navigate}>
              {ownerLabel}
            </ConsoleLink>
            <span aria-hidden="true">/</span>
            <strong className="font-medium text-foreground">{project?.name ?? projectId}</strong>
          </nav>
          <div className="grid gap-1">
            <Eyebrow>{SECTION_EYEBROW[section]}</Eyebrow>
            <h1 id="project-title" className="text-2xl">
              {project?.name ?? "Loading…"}
            </h1>
            {project === null ? null : (
              <p className="m-0 mt-1 flex flex-wrap items-center gap-3 text-sm text-muted-foreground">
                <LifecycleBadge state={project.state} />
                <span>Region {project.region}</span>
                <code className="font-mono text-xs">{project.id}</code>
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
        <section aria-labelledby="selected-environment-title" className="grid gap-4">
          <div className="flex flex-wrap items-start justify-between gap-4">
            <div className="grid gap-1">
              <Eyebrow>Selected environment</Eyebrow>
              <h2 id="selected-environment-title" className="text-xl">
                {selectedEnvironment.name}
              </h2>
              <p className="m-0 text-sm text-muted-foreground">
                Each summary loads on its own and names when it was observed. An unavailable summary
                never hides a healthy one.
              </p>
            </div>
            <Button variant="outline" size="sm" onClick={() => setRefreshKey((value) => value + 1)}>
              <RefreshCw aria-hidden="true" />
              Refresh summaries
            </Button>
          </div>
          <div
            className="grid gap-4 xl:grid-cols-2"
            key={`${selectedEnvironment.id}:${refreshKey}`}
          >
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
    <Card aria-labelledby="environments-title">
      <CardHeader>
        <CardTitle id="environments-title">Environments</CardTitle>
        <CardDescription>
          Each environment reports its own readiness. Select one to see its keys, usage, health, and
          activity.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {environments.length === 0 ? (
          <EmptyState
            icon={<Layers aria-hidden="true" />}
            title="This project has no environments yet. Create one to start connecting clients."
          />
        ) : (
          <div className="overflow-hidden rounded-lg border">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col" className="w-10 pl-3">
                    <span className="sr-only">Selected</span>
                  </TableHead>
                  <TableHead scope="col">Environment</TableHead>
                  <TableHead scope="col">State</TableHead>
                  <TableHead scope="col">Readiness</TableHead>
                  <TableHead scope="col">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
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
              </TableBody>
            </Table>
          </div>
        )}
        <details>
          <summary className="flex w-fit cursor-pointer list-none items-center gap-1.5 text-sm font-medium text-primary hover:underline [&::-webkit-details-marker]:hidden">
            <Plus aria-hidden="true" className="size-4" />
            Create environment
          </summary>
          <form
            className="mt-3 flex max-w-xl flex-wrap items-end gap-3"
            onSubmit={(event) => void createEnvironment(event)}
          >
            <Field
              label="Environment name"
              htmlFor="new-environment-name"
              className="min-w-56 flex-1"
            >
              <Input id="new-environment-name" name="name" required maxLength={100} />
            </Field>
            <Button type="submit">Create environment</Button>
          </form>
        </details>
      </CardContent>
    </Card>
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
    <TableRow
      aria-selected={selected}
      data-state={selected ? "selected" : undefined}
      className="align-top"
    >
      <TableCell className="pl-3">
        <Input
          type="radio"
          name="selected-environment"
          aria-label={`Select ${environment.name}`}
          checked={selected}
          onChange={onSelect}
          className="mt-0.5 h-4 w-4 cursor-pointer rounded-full border-0 p-0 shadow-none accent-primary"
        />
      </TableCell>
      <TableCell className="whitespace-normal">
        <strong className="font-medium">{environment.name}</strong>
        <br />
        <code className="font-mono text-xs text-muted-foreground">{environment.id}</code>
        {environment.deletionDeadline === undefined ? null : (
          <small className={OBSERVED}>
            Restorable until {new Date(environment.deletionDeadline).toLocaleString()}
          </small>
        )}
      </TableCell>
      <TableCell>
        <LifecycleBadge state={environment.state} />
      </TableCell>
      <TableCell className="whitespace-normal">
        <EnvironmentReadiness projectId={environment.projectId} environmentId={environment.id} />
      </TableCell>
      <TableCell className="whitespace-normal">
        <div className="flex flex-wrap items-center gap-1">
          <ConsoleLink
            className="px-2 text-sm font-medium"
            path={`/projects/${environment.projectId}/environments/${environment.id}/overview`}
            navigate={navigate}
          >
            Open
          </ConsoleLink>
          <Button
            variant="ghost"
            size="sm"
            disabled={environment.state !== "active"}
            onClick={() => void act("suspend")}
          >
            Suspend
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={!(["suspended", "deletion_grace"] as string[]).includes(environment.state)}
            onClick={() => void act("restore")}
          >
            Restore
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="text-destructive hover:text-destructive"
            disabled={!(["active", "suspended", "failed"] as string[]).includes(environment.state)}
            onClick={() => void act("delete")}
          >
            Delete
          </Button>
        </div>
        <ApiFailureNotice failure={failure} />
      </TableCell>
    </TableRow>
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
    return <span className="text-xs text-muted-foreground">Loading readiness…</span>;
  }
  if (summary.status === "unavailable") {
    return (
      <div className="grid gap-1">
        <StatusBadge state="unavailable" />
        <small className={OBSERVED}>{summary.failure.message}</small>
      </div>
    );
  }
  const section = summary.value.sections.lifecycle ?? summary.value.sections.readiness;
  if (section === undefined) {
    return (
      <div className="grid gap-1">
        <StatusBadge state="unavailable" />
        <small className={OBSERVED}>
          No readiness section · observed {summary.observedAt.toLocaleString()}
        </small>
      </div>
    );
  }
  const detail = readinessDetail(section.payload);
  const counts = summaryCounts(summary.value);
  return (
    <div className="grid gap-1">
      <span className="flex flex-wrap items-center gap-2">
        <StatusBadge state={section.status} />
        {detail === null ? null : <span>{detail}</span>}
      </span>
      {counts === null ? null : <small className="block text-xs">{counts}</small>}
      <small className={OBSERVED}>
        Observed {formatUnixSeconds(section.observedAtUnixSeconds)}
      </small>
      {section.remediationCode === null || section.remediationCode === undefined ? null : (
        <small className={OBSERVED}>{humanize(section.remediationCode)}</small>
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
      <dl className="m-0 grid gap-3 sm:grid-cols-2">
        <Definition term="API URL" className="sm:col-span-2">
          <code className="font-mono break-all">{metadata.publicEndpoint}</code>
        </Definition>
        <Definition term="Public key ID">
          <code className="font-mono break-all">{metadata.publicKeyId}</code>
        </Definition>
        <Definition term="Supported client">
          <code className="font-mono">{metadata.rxdbClientRange}</code>
        </Definition>
      </dl>
      {metadata.publicKey === "" ? (
        <Alert variant="warning">
          <AlertDescription className="block">
            No recoverable public key is available. Issue or rotate a public project credential on
            the Credentials page, then paste the one-time value into your application secret store.
          </AlertDescription>
        </Alert>
      ) : (
        <Field label="Public project key" htmlFor="public-project-key">
          <Input
            id="public-project-key"
            readOnly
            value={metadata.publicKey}
            className="font-mono text-xs md:text-xs"
          />
        </Field>
      )}
      <div className="grid gap-2">
        <p className="m-0 flex flex-wrap items-baseline gap-x-3 gap-y-1 text-sm">
          <strong className="font-medium">Quickstart</strong>
          {collection === undefined ? (
            <small className="text-xs text-muted-foreground">
              No collection exists yet; the snippet uses a placeholder collection.
            </small>
          ) : (
            <small className="text-xs text-muted-foreground">
              Collection {collection.collectionId} · schema v{collection.activeSchemaVersion}
            </small>
          )}
        </p>
        <div className="relative rounded-lg border bg-muted/50">
          <Button
            variant="outline"
            size="sm"
            className="absolute top-2 right-2"
            onClick={() => void navigator.clipboard.writeText(snippet)}
          >
            <Copy aria-hidden="true" />
            Copy
          </Button>
          <pre className="m-0 overflow-x-auto p-4 pr-24 font-mono text-xs leading-relaxed break-words whitespace-pre-wrap">
            {snippet}
          </pre>
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
          <p className="m-0 text-sm text-muted-foreground">
            No usage has been recorded in the retention window.
          </p>
        ) : (
          <div className="overflow-hidden rounded-lg border">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col">Resource</TableHead>
                  <TableHead scope="col" className="text-right">
                    Quantity
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {totals.map((total) => (
                  <TableRow key={total.resource}>
                    <TableCell>{total.resource.replaceAll("_", " ")}</TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatQuantity(total.resource, total.quantity)}
                      {total.resource.includes("bytes") ? "" : ` ${total.unit}`}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
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
          <p className="m-0 text-sm text-muted-foreground">No retained health observations.</p>
        ) : (
          <ul className="m-0 grid list-none gap-2 p-0 text-sm">
            {health.map((record) => (
              <li key={`${record.timestamp}-${record.payload.service}-${record.payload.region}`}>
                <strong className="font-medium">{record.payload.service}</strong> in{" "}
                {record.payload.region}: {record.payload.status}
                {record.payload.diagnostic === null ? null : (
                  <small className={OBSERVED}>{record.payload.diagnostic}</small>
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
          <p className="m-0 text-sm text-muted-foreground">
            No audited actions in the retention window.
          </p>
        ) : (
          <ol className="m-0 grid list-none gap-3 p-0 text-sm">
            {events.map((event) => (
              <li
                key={`${event.timestamp}-${event.requestId}`}
                className="grid gap-0.5 border-b pb-3 last:border-0 last:pb-0"
              >
                <time dateTime={event.timestamp} className="text-xs text-muted-foreground">
                  {new Date(event.timestamp).toLocaleString()}
                </time>
                <span>
                  <strong className="font-medium">{event.actorId}</strong> {event.action}{" "}
                  <code className="font-mono text-xs">{event.target}</code>
                </span>
                <small className={OBSERVED}>
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
    <Card aria-labelledby="lifecycle-title">
      <CardHeader>
        <CardTitle id="lifecycle-title">Provisioning and lifecycle</CardTitle>
        <CardDescription>
          Current state:{" "}
          <strong className="font-medium text-foreground">
            {project.state.replaceAll("_", " ")}
          </strong>
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-3">
        {project.failureDiagnostic === undefined ? null : (
          <Alert variant="destructive">
            <AlertDescription className="block">{project.failureDiagnostic}</AlertDescription>
          </Alert>
        )}
        {project.deletionDeadline === undefined ? null : (
          <p className="m-0 text-sm">
            Restorable until {new Date(project.deletionDeadline).toLocaleString()}.
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            onClick={() => onAction("suspend")}
            disabled={project.state !== "active"}
          >
            Suspend
          </Button>
          <Button
            variant="outline"
            onClick={() => onAction("restore")}
            disabled={!(["suspended", "deletion_grace"] as string[]).includes(project.state)}
          >
            Restore
          </Button>
          <Button
            variant="destructive"
            onClick={() => onAction("delete")}
            disabled={!(["active", "suspended", "failed"] as string[]).includes(project.state)}
          >
            Request deletion
          </Button>
        </div>
      </CardContent>
    </Card>
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
      <Card aria-labelledby="identifiers-title">
        <CardHeader>
          <CardTitle id="identifiers-title">Identifiers and ownership</CardTitle>
        </CardHeader>
        <CardContent>
          <dl className="m-0 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <Definition term="Project ID">
              <code className="font-mono break-all">{project.id}</code>
            </Definition>
            <Definition term="Owner">
              {team === null
                ? ownerUnavailable
                  ? "Owner unavailable"
                  : "Loading…"
                : team.kind === "personal"
                  ? "Your personal space"
                  : team.name}
            </Definition>
            <Definition term="Owner kind">
              {team === null ? "—" : team.kind === "personal" ? "Personal space" : "Team"}
            </Definition>
            <Definition term="Owner ID">
              <code className="font-mono break-all">{project.teamId}</code>
            </Definition>
            <Definition term="Region">{project.region}</Definition>
            <Definition term="Lifecycle">
              <LifecycleBadge state={project.state} />
            </Definition>
            <Definition term="Created">{new Date(project.createdAt).toLocaleString()}</Definition>
            <Definition term="Updated">{new Date(project.updatedAt).toLocaleString()}</Definition>
          </dl>
        </CardContent>
      </Card>
      <RenameProjectPanel project={project} onChanged={onChanged} />
      <TransferProjectPanel project={project} owner={team} onChanged={onChanged} />
      <Card aria-labelledby="deletion-title" className="border-destructive/30">
        <CardHeader>
          <CardTitle id="deletion-title">Deletion</CardTitle>
          <CardDescription>
            Requesting deletion revokes data-plane access immediately. Final destruction follows a
            grace period during which the project can be restored.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3">
          {project.failureDiagnostic === undefined ? null : (
            <Alert variant="destructive">
              <AlertDescription className="block">{project.failureDiagnostic}</AlertDescription>
            </Alert>
          )}
          {project.deletionDeadline === undefined ? null : (
            <p className="m-0 text-sm">
              Restorable until {new Date(project.deletionDeadline).toLocaleString()}.
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              variant="destructive"
              onClick={() => onAction("delete")}
              disabled={!(["active", "suspended", "failed"] as string[]).includes(project.state)}
            >
              Request deletion
            </Button>
            <Button
              variant="outline"
              onClick={() => onAction("restore")}
              disabled={project.state !== "deletion_grace"}
            >
              Restore
            </Button>
          </div>
        </CardContent>
      </Card>
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
    <Card aria-labelledby="rename-title">
      <CardHeader>
        <CardTitle id="rename-title">Project name</CardTitle>
        <CardDescription>
          The name appears everywhere the project is listed. Its identifier, keys, policies, and
          data never change with it. Renaming requires confirmation and is audited.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-3">
        <form
          className="flex max-w-2xl flex-wrap items-end gap-3"
          onSubmit={(event) => void submit(event)}
        >
          <Field label="Project name" htmlFor="project-name" className="min-w-64 flex-1">
            <Input
              id="project-name"
              name="name"
              value={name}
              maxLength={200}
              onChange={(event) => setDraft(event.currentTarget.value)}
            />
          </Field>
          <Button type="submit" disabled={unchanged || pending}>
            {pending ? "Renaming…" : "Rename"}
          </Button>
        </form>
        <ApiFailureNotice failure={failure} />
        {status === null ? null : (
          <Alert variant="positive" role="status">
            <AlertDescription className="block">{status}</AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
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
    form = (
      <p aria-busy="true" className="m-0 text-sm text-muted-foreground">
        Loading the teams you belong to…
      </p>
    );
  } else if (teams.status === "unavailable") {
    form = (
      <>
        <p className="m-0 text-sm">
          The teams you belong to could not be listed, so no transfer can be offered right now.
        </p>
        <ApiFailureNotice failure={teams.failure} />
      </>
    );
  } else if (ownerIsPersonal && teamTargets.length === 0) {
    form = (
      <p className="m-0 text-sm">
        This project is in your personal space and you belong to no team, so there is no owner to
        transfer it to.
      </p>
    );
  } else {
    form = (
      <form
        className="flex max-w-2xl flex-wrap items-end gap-3"
        onSubmit={(event) => void submit(event)}
      >
        <Field label="Transfer to" htmlFor="transfer-target" className="min-w-64 flex-1">
          <NativeSelect
            id="transfer-target"
            value={selection}
            onChange={(event) => setSelection(event.currentTarget.value)}
          >
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
          </NativeSelect>
        </Field>
        <Button type="submit" disabled={target === null || pending}>
          {pending ? "Transferring…" : "Transfer"}
        </Button>
      </form>
    );
  }
  return (
    <Card aria-labelledby="transfer-title">
      <CardHeader>
        <CardTitle id="transfer-title">Transfer ownership</CardTitle>
        <CardDescription>
          Move this project to your personal space or to a team you administer. Its identifier,
          environments, policies, users, data, and keys stay as they are; usage limits follow the
          new owner's plan. A transfer requires confirmation and is audited under both owners.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-3">
        {form}
        <ApiFailureNotice failure={failure} />
        {status === null ? null : (
          <Alert variant="positive" role="status">
            <AlertDescription className="block">{status}</AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
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
    <Card aria-labelledby={`${id}-summary-title`} data-status={state.status} className="gap-4">
      <CardHeader>
        <CardTitle id={`${id}-summary-title`}>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
        <CardAction>
          <StatusBadge state={badge} />
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-3">
        {state.status === "loading" ? (
          <p aria-busy="true" className="m-0 text-sm text-muted-foreground">
            Loading…
          </p>
        ) : state.status === "unavailable" ? (
          <>
            <p className="m-0 text-sm text-muted-foreground">
              This summary is unavailable; the other summaries are unaffected.
            </p>
            <ApiFailureNotice failure={state.failure} />
          </>
        ) : (
          <>
            {children(state.value)}
            <small className={OBSERVED}>
              Observed {observedAt(state.value, state.observedAt).toLocaleString()}
            </small>
          </>
        )}
      </CardContent>
      {actions === undefined ? null : (
        <CardFooter className="flex-wrap gap-4 text-sm font-medium">{actions}</CardFooter>
      )}
    </Card>
  );
}

/** One term and its value in a definition list laid out as a grid. */
function Definition({
  term,
  className,
  children,
}: {
  readonly term: string;
  readonly className?: string | undefined;
  readonly children: ReactNode;
}) {
  return (
    <div className={cn("grid gap-1", className)}>
      <dt className="text-xs font-medium text-muted-foreground">{term}</dt>
      <dd className="m-0 text-sm">{children}</dd>
    </div>
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

/// A summary's freshness or an environment's readiness as one word. The
/// `status-badge` class is the hook the browser suites select it by.
function StatusBadge({ state }: { readonly state: string }) {
  const variant =
    state === "current" || state === "ready" || state === "active"
      ? "positive"
      : state === "unavailable" || state === "failed"
        ? "destructive"
        : state === "loading"
          ? "outline"
          : "warning";
  return (
    <Badge
      className={cn("status-badge", variant === "outline" && "text-muted-foreground")}
      variant={variant}
    >
      {humanize(state)}
    </Badge>
  );
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
