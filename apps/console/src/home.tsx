// Home dashboard and first-run onboarding.
//
// The dashboard lists projects per owner, then loads plans and usage through
// a bounded request queue. A failed summary marks only that project's row.
// First-run onboarding creates a project and opens its workspace. Progress
// is stored per developer in browser storage, without credential values.

import type {
  Environment,
  MakoManagementClient,
  ObservabilityRecord,
  Project,
  Team,
} from "@mako-cloud/management-sdk";
import {
  Badge,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  cn,
  Eyebrow,
  Field,
  Input,
  NativeSelect,
} from "@mako-cloud/ui";
import { Database, FolderOpen, Gauge, Plus, Users } from "lucide-react";
import {
  type FormEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useDeveloperAuth } from "./auth.js";
import { BillingPanel } from "./billing.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { CreateTeamForm } from "./teams.js";

/** Summary requests in flight at once across every project on the page. */
const MAX_IN_FLIGHT = 4;
/** Usage samples read per project; the newest level and the period's flows come out of them. */
const USAGE_SAMPLE_LIMIT = 50;
/** A small label over a fact. `dt` keeps the definition list intact, so this is
 * the kit's Eyebrow styling on the term rather than the component. */
const FACT_LABEL = "text-xs font-semibold uppercase tracking-wider text-muted-foreground";

/** A quiet line of text: a loading note, an empty list, a caption. */
const MUTED = "m-0 text-sm text-muted-foreground";

export function HomeDashboard({
  onOpenTeam,
  onOpenProject,
  navigate,
  view = "projects",
}: {
  readonly view?: "projects" | "usage";
  readonly navigate: (path: string) => void;
  readonly onOpenTeam: (teamId: string) => void;
  readonly onOpenProject: (projectId: string) => void;
}) {
  const client = useManagementClient();
  const { state } = useDeveloperAuth();
  const developerId = state.status === "authenticated" ? state.session.profile.id : "";
  const [creating, setCreating] = useState(false);
  const [teams, setTeams] = useState<Team[] | null>(null);
  const [groups, setGroups] = useState<Record<string, ProjectsLoad>>({});
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [progress, setProgress] = useOnboardingProgress(developerId);
  const loader = useMemo(() => createSummaryLoader(client), [client]);

  const reload = useCallback(async () => {
    setFailure(null);
    let list: Team[];
    try {
      list = sortOwners(await client.listTeams());
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
      setTeams([]);
      setGroups({});
      return;
    }
    setTeams(list);
    setGroups(Object.fromEntries(list.map((team) => [team.id, { status: "loading" as const }])));
    await Promise.all(
      list.map(async (team) => {
        let load: ProjectsLoad;
        try {
          load = { status: "ready", projects: await client.listProjects(team.id) };
        } catch (error) {
          load = { status: "failed", failure: toConsoleApiFailure(error) };
        }
        setGroups((current) => ({ ...current, [team.id]: load }));
      }),
    );
  }, [client]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const personalSpace = teams?.find((team) => team.kind === "personal");
  const joinedTeams = teams?.filter((team) => team.kind === "team") ?? [];
  const loading =
    teams === null || teams.some((team) => (groups[team.id]?.status ?? "loading") === "loading");
  const readyProjects = (teams ?? []).flatMap((team) => {
    const load = groups[team.id];
    return load?.status === "ready" ? load.projects : [];
  });
  // "No reachable projects" is only known once every owner answered; a failed
  // listing is reported in place rather than mistaken for an empty account.
  const noProjects =
    failure === null &&
    (teams?.every((team) => groups[team.id]?.status === "ready") ?? false) &&
    readyProjects.length === 0;
  const guideInFlight = progress.step !== "done" && progress.projectId !== undefined;
  const guideActive = !progress.dismissed && (noProjects || guideInFlight);
  const showGuideAgain = progress.dismissed && (noProjects || guideInFlight);
  const ownerName = (project: Project): string =>
    teams?.find((team) => team.id === project.teamId)?.kind === "personal"
      ? "Personal"
      : (teams?.find((team) => team.id === project.teamId)?.name ?? project.teamId);

  const completeGuide = () => {
    setProgress({ ...progress, step: "done", dismissed: false });
    void reload();
  };

  return (
    <div className="grid min-h-[calc(100vh-3.5rem)] md:grid-cols-[15rem_minmax(0,1fr)]">
      <aside
        aria-label="Workspace navigation"
        className="flex flex-col gap-6 border-b bg-sidebar p-4 md:border-r md:border-b-0"
      >
        <div className="flex items-center gap-3 px-2 py-2">
          <div className="grid size-9 place-items-center rounded-lg bg-primary/10 text-primary">
            <Database className="size-5" aria-hidden="true" />
          </div>
          <div>
            <strong className="text-sm">Your workspace</strong>
            <p className="m-0 text-xs text-muted-foreground">Mako Cloud</p>
          </div>
        </div>
        <nav aria-label="Workspace destinations" className="grid gap-1">
          <a
            href="/"
            aria-current={view === "projects" ? "page" : undefined}
            className={cn(
              "flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-foreground no-underline hover:bg-sidebar-accent",
              view === "projects" && "bg-sidebar-accent",
            )}
          >
            <FolderOpen className="size-4" aria-hidden="true" />
            Personal projects
          </a>
          <a
            href="/usage-and-plan"
            aria-current={view === "usage" ? "page" : undefined}
            className={cn(
              "flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-foreground no-underline hover:bg-sidebar-accent",
              view === "usage" && "bg-sidebar-accent",
            )}
          >
            <Gauge className="size-4" aria-hidden="true" />
            Usage and plan
          </a>
        </nav>
        <nav aria-label="Teams" className="grid gap-2">
          <Eyebrow className="px-3">Teams</Eyebrow>
          {joinedTeams.map((team) => (
            <a
              key={team.id}
              href={`/teams/${team.id}`}
              className="flex items-center gap-2 rounded-md px-3 py-2 text-sm text-foreground no-underline hover:bg-sidebar-accent"
              onClick={(event) => {
                if (
                  event.button !== 0 ||
                  event.metaKey ||
                  event.ctrlKey ||
                  event.shiftKey ||
                  event.altKey
                )
                  return;
                event.preventDefault();
                onOpenTeam(team.id);
              }}
            >
              <Users className="size-4 shrink-0" aria-hidden="true" />
              {team.name}
            </a>
          ))}
          {teams !== null && failure === null && joinedTeams.length === 0 ? (
            <p className="m-0 px-3 text-xs text-muted-foreground">No teams yet.</p>
          ) : null}
          {teams !== null && failure === null ? (
            <CreateTeamForm onCreated={(team) => onOpenTeam(team.id)} />
          ) : null}
        </nav>
        <p className="mt-auto hidden border-t px-3 pt-4 text-xs leading-5 text-muted-foreground md:block">
          Each project has isolated environments, collections, and application credentials.
        </p>
      </aside>
      {view === "usage" ? (
        <section
          className="grid min-w-0 content-start gap-6 p-5 lg:p-8"
          aria-labelledby="overall-usage-title"
        >
          <div className="grid gap-1">
            <Eyebrow>Your workspace</Eyebrow>
            <h1 id="overall-usage-title" className="text-2xl">
              Usage and plan
            </h1>
            <p className={MUTED}>
              Combined usage, shared allowances, and plan costs for your personal projects and each
              team.
            </p>
          </div>
          <ApiFailureNotice failure={failure} />
          {teams === null ? <p className={MUTED}>Loading plans…</p> : null}
          {teams?.length === 0 && failure === null ? (
            <p className={MUTED}>Create your first project to start tracking usage.</p>
          ) : null}
          {teams?.map((team) => {
            const load = groups[team.id];
            return (
              <section
                key={team.id}
                className="grid min-w-0 gap-4"
                aria-labelledby={`plan-${team.id}`}
              >
                <h2 id={`plan-${team.id}`} className="text-lg">
                  {team.kind === "personal" ? "Personal projects" : team.name}
                </h2>
                <BillingPanel teamId={team.id} />
                {load?.status === "failed" ? <ApiFailureNotice failure={load.failure} /> : null}
                <div className="flex flex-wrap gap-3">
                  {readyProjects
                    .filter((project) => project.teamId === team.id)
                    .map((project) => (
                      <a
                        key={project.id}
                        href={`/projects/${project.id}/billing`}
                        className="text-sm text-primary underline underline-offset-4"
                      >
                        {project.name} billing
                      </a>
                    ))}
                </div>
              </section>
            );
          })}
        </section>
      ) : (
        <section
          className="grid min-w-0 content-start gap-6 p-5 lg:p-8"
          aria-labelledby="home-title"
          aria-busy={loading}
        >
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div className="grid gap-1">
              <Eyebrow>Cloud databases</Eyebrow>
              <h1 id="home-title" className="text-2xl">
                Home
              </h1>
              <p className="m-0 text-sm text-muted-foreground">
                Manage your databases, connect applications, and inspect your data.
              </p>
            </div>
            {!loading && !noProjects && !guideActive && failure === null ? (
              <Button onClick={() => setCreating((value) => !value)} aria-expanded={creating}>
                <Plus aria-hidden="true" />
                Create project
              </Button>
            ) : null}
          </div>
          {creating ? (
            <Card aria-label="New project">
              <CardHeader>
                <CardTitle>Create a database project</CardTitle>
                <CardDescription>
                  Choose an owner and region. Your first environment is provisioned with the
                  project.
                </CardDescription>
              </CardHeader>
              <CardContent>
                <ProjectCreateForm
                  teams={joinedTeams}
                  submitLabel="Create and provision"
                  onCreated={(project) => onOpenProject(project.id)}
                />
                <Button variant="ghost" className="mt-3" onClick={() => setCreating(false)}>
                  Cancel
                </Button>
              </CardContent>
            </Card>
          ) : null}
          <ApiFailureNotice failure={failure} />
          {teams === null ? (
            <p className={MUTED} aria-live="polite">
              Loading your projects…
            </p>
          ) : null}
          {guideActive ? (
            <FirstRunGuide
              teams={joinedTeams}
              progress={progress}
              onProgress={setProgress}
              onOpenProject={onOpenProject}
              onCompleted={completeGuide}
            />
          ) : null}
          {noProjects && !guideActive ? (
            <NoProjectsPanel
              teams={joinedTeams}
              onOpenProject={onOpenProject}
              onShowGuide={() => setProgress({ ...progress, dismissed: false })}
            />
          ) : null}
          {teams !== null && !noProjects && failure === null ? (
            <>
              <div className="grid items-start gap-6">
                <div className="grid min-w-0 gap-8">
                  {personalSpace === undefined ? (
                    <section className="grid gap-4" aria-labelledby="owner-personal-title">
                      <div className="grid gap-1">
                        <Eyebrow>Projects</Eyebrow>
                        <h2 id="owner-personal-title" className="text-lg">
                          Personal projects
                        </h2>
                      </div>
                      <p className={MUTED}>No personal projects yet.</p>
                    </section>
                  ) : (
                    <OwnerGroup
                      team={personalSpace}
                      load={groups[personalSpace.id] ?? { status: "loading" }}
                      loader={loader}
                      ownerName={ownerName}
                      onOpenProject={onOpenProject}
                      onOpenTeam={onOpenTeam}
                      navigate={navigate}
                    />
                  )}
                </div>
              </div>
              <section className="grid gap-4" aria-labelledby="teams-title">
                <div className="grid gap-1">
                  <Eyebrow>Workspace</Eyebrow>
                  <h2 id="teams-title" className="text-lg">
                    Teams
                  </h2>
                </div>
                {joinedTeams.length === 0 ? (
                  <p className={MUTED}>No teams are available for this account.</p>
                ) : (
                  <div className="grid min-w-0 gap-6">
                    {joinedTeams.map((team) => (
                      <OwnerGroup
                        key={team.id}
                        team={team}
                        load={groups[team.id] ?? { status: "loading" }}
                        loader={loader}
                        ownerName={ownerName}
                        onOpenProject={onOpenProject}
                        onOpenTeam={onOpenTeam}
                        navigate={navigate}
                      />
                    ))}
                  </div>
                )}
              </section>
            </>
          ) : null}
          {showGuideAgain && !noProjects ? (
            <p className="m-0">
              <Button
                variant="link"
                className="h-auto p-0"
                onClick={() => setProgress({ ...progress, dismissed: false })}
              >
                Show the guide again
              </Button>
            </p>
          ) : null}
        </section>
      )}
    </div>
  );
}

// --- Project groups and rows -------------------------------------------------

type ProjectsLoad =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly projects: readonly Project[] }
  | { readonly status: "failed"; readonly failure: ConsoleApiFailure };

function OwnerGroup({
  team,
  load,
  loader,
  ownerName,
  onOpenProject,
  onOpenTeam,
  navigate,
}: {
  readonly navigate: (path: string) => void;
  readonly team: Team;
  readonly load: ProjectsLoad;
  readonly loader: SummaryLoader;
  readonly ownerName: (project: Project) => string;
  readonly onOpenProject: (projectId: string) => void;
  readonly onOpenTeam: (teamId: string) => void;
}) {
  const personal = team.kind === "personal";
  const headingId = `owner-${team.id}-title`;
  return (
    <section
      className="grid gap-4"
      aria-labelledby={headingId}
      aria-busy={load.status === "loading"}
    >
      <div className="grid gap-1">
        {personal ? (
          <>
            <Eyebrow>Projects</Eyebrow>
            <h2 id={headingId} className="text-lg">
              Personal projects
            </h2>
          </>
        ) : (
          <h3 id={headingId} className="m-0 text-base font-semibold">
            <Button
              variant="link"
              className="h-auto max-w-full p-0 text-left text-base text-foreground whitespace-normal break-words"
              onClick={() => onOpenTeam(team.id)}
            >
              {team.name}
            </Button>
          </h3>
        )}
      </div>
      {load.status === "loading" ? (
        <p className={MUTED} aria-live="polite">
          Loading projects…
        </p>
      ) : load.status === "failed" ? (
        <ApiFailureNotice failure={load.failure} />
      ) : load.projects.length === 0 ? (
        <p className={MUTED}>
          {personal ? "No personal projects yet." : "This team has no projects yet."}
        </p>
      ) : (
        <ul
          aria-labelledby={headingId}
          className="m-0 grid list-none divide-y overflow-hidden rounded-lg border bg-card p-0"
        >
          {load.projects.map((project) => (
            <li key={project.id} className="min-w-0">
              <ProjectRow
                project={project}
                owner={ownerName(project)}
                loader={loader}
                navigate={navigate}
                onOpen={() => onOpenProject(project.id)}
              />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

type Summary<T> =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly value: T }
  | { readonly status: "unavailable" };

function ProjectRow({
  project,
  owner,
  loader,
  onOpen,
  navigate,
}: {
  readonly navigate: (path: string) => void;
  readonly project: Project;
  readonly owner: string;
  readonly loader: SummaryLoader;
  readonly onOpen: () => void;
}) {
  const [environment, setEnvironment] = useState<Environment | null>(null);
  const [plan, setPlan] = useState<Summary<string>>({ status: "loading" });
  const [usage, setUsage] = useState<Summary<UsageSummary>>({ status: "loading" });
  useEffect(() => {
    let active = true;
    setPlan({ status: "loading" });
    setEnvironment(null);
    void loader.environment(project.id).then(
      (value) => active && setEnvironment(value),
      () => {},
    );
    setUsage({ status: "loading" });
    void loader.plan(project.teamId).then(
      (value) => active && setPlan({ status: "ready", value }),
      () => active && setPlan({ status: "unavailable" }),
    );
    void loader.usage(project.id).then(
      (value) => active && setUsage({ status: "ready", value }),
      () => active && setUsage({ status: "unavailable" }),
    );
    return () => {
      active = false;
    };
  }, [loader, project.id, project.teamId]);
  const headingId = `project-${project.id}-title`;
  return (
    <article
      className="grid min-w-0 gap-4 p-4 transition-colors hover:bg-muted/30 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center"
      aria-labelledby={headingId}
      aria-busy={plan.status === "loading" || usage.status === "loading"}
    >
      <div className="grid min-w-0 gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)] xl:items-center">
        <div className="grid min-w-0 gap-1.5">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <h3 id={headingId} className="m-0 min-w-0 text-sm font-semibold">
              <Button
                variant="link"
                className="h-auto max-w-full p-0 text-left text-foreground whitespace-normal break-words"
                onClick={onOpen}
              >
                {project.name}
              </Button>
            </h3>
            <LifecycleBadge state={project.state} />
          </div>
          <span className="text-xs text-muted-foreground">{owner}</span>
          <code className="break-all text-xs text-muted-foreground">{project.id}</code>
        </div>
        <dl className="m-0 grid min-w-0 grid-cols-2 gap-x-4 gap-y-2 text-sm">
          <Fact label="Region">
            <span className="font-mono text-xs">{project.region}</span>
          </Fact>
          <Fact label="Plan">
            {plan.status === "loading" ? (
              <span className="text-muted-foreground italic">Loading plan…</span>
            ) : plan.status === "unavailable" ? (
              <Badge variant="outline" className="text-destructive">
                Plan unavailable
              </Badge>
            ) : (
              plan.value
            )}
          </Fact>
          <Fact label="Usage" className="col-span-2">
            {usage.status === "loading" ? (
              <span className="text-muted-foreground italic">Loading usage…</span>
            ) : usage.status === "unavailable" ? (
              <Badge variant="outline" className="text-destructive">
                Usage unavailable
              </Badge>
            ) : (
              <UsageLine usage={usage.value} />
            )}
          </Fact>
        </dl>
      </div>
      <div className="grid min-w-0 gap-2 lg:justify-items-end">
        <a
          href={`/projects/${project.id}/billing`}
          className="text-sm text-primary underline underline-offset-4"
        >
          Billing
        </a>
        {environment === null ? null : (
          <div className="grid min-w-0 gap-2 lg:justify-items-end">
            <span className="break-words text-xs text-muted-foreground">
              Database · {environment.name}
            </span>
            <div className="flex flex-wrap gap-2">
              {[
                ["data", "Browse data"],
                ["collections", "Schema"],
                ["connect", "Connect"],
              ].map(([section, label]) => (
                <Button
                  key={section}
                  size="sm"
                  variant={section === "data" ? "default" : "outline"}
                  onClick={() =>
                    navigate(`/projects/${project.id}/environments/${environment.id}/${section}`)
                  }
                >
                  {section === "data" ? <Database aria-hidden="true" /> : null}
                  {label}
                </Button>
              ))}
            </div>
          </div>
        )}
      </div>
    </article>
  );
}

/** One project fact: its name over its value. */
function Fact({
  label,
  className,
  children,
}: {
  readonly label: string;
  readonly className?: string;
  readonly children: ReactNode;
}) {
  return (
    <div className={cn("grid min-w-0 content-start gap-0.5", className)}>
      <dt className={FACT_LABEL}>{label}</dt>
      <dd className="m-0 break-words">{children}</dd>
    </div>
  );
}

function UsageLine({ usage }: { readonly usage: UsageSummary }) {
  if (usage.environment === null) {
    return <span className="text-muted-foreground">No active environment</span>;
  }
  const parts = [
    ...(usage.storageBytes === null ? [] : [`Storage ${formatBytes(usage.storageBytes)}`]),
    ...(usage.replicationBytes === null
      ? []
      : [`Replication ${formatBytes(usage.replicationBytes)}`]),
  ];
  return (
    <span>
      <span className="font-medium">{usage.environment.name}</span>
      {parts.length === 0 ? " · No usage recorded this period" : ` · ${parts.join(" · ")}`}
    </span>
  );
}

// --- Empty state --------------------------------------------------------------

function NoProjectsPanel({
  teams,
  onOpenProject,
  onShowGuide,
}: {
  readonly teams: readonly Team[];
  readonly onOpenProject: (projectId: string) => void;
  readonly onShowGuide: () => void;
}) {
  return (
    <Card aria-labelledby="empty-title">
      <CardHeader>
        <Eyebrow>No projects yet</Eyebrow>
        <CardTitle id="empty-title">Create a project</CardTitle>
        <CardDescription>
          A project holds environments with their collections, application users, policies, and
          functions. Individual projects live in your personal space; team projects belong to the
          team that owns them.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ProjectCreateForm
          teams={teams}
          submitLabel="Create project"
          onCreated={(project) => onOpenProject(project.id)}
        />
        <p className="m-0">
          <Button variant="link" className="h-auto p-0" onClick={onShowGuide}>
            Show the guide again
          </Button>
        </p>
      </CardContent>
    </Card>
  );
}

/**
 * Creates a project in the personal space or a chosen team. The personal
 * space is selected by omitting `teamId`; the server resolves the caller's
 * space and creates it on first use.
 */
function ProjectCreateForm({
  teams,
  submitLabel,
  onCreated,
}: {
  readonly teams: readonly Team[];
  readonly submitLabel: string;
  readonly onCreated: (project: Project) => void;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  const id = useId();
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const teamId = String(data.get("owner") ?? "");
    const name = String(data.get("name") ?? "").trim();
    const region = String(data.get("region") ?? "").trim();
    setPending(true);
    setFailure(null);
    try {
      const project = await client.createProject(
        { ...(teamId === "" ? {} : { teamId }), name, region },
        idempotencyKey(),
      );
      onCreated(project);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
      setPending(false);
    }
  };
  return (
    <form className="grid max-w-md gap-4" onSubmit={(event) => void submit(event)}>
      {teams.length === 0 ? null : (
        <Field label="Owner" htmlFor={`${id}-owner`}>
          <NativeSelect id={`${id}-owner`} name="owner" defaultValue="">
            <option value="">Personal</option>
            {teams.map((team) => (
              <option key={team.id} value={team.id}>
                {team.name}
              </option>
            ))}
          </NativeSelect>
        </Field>
      )}
      <Field label="Project name" htmlFor={`${id}-name`}>
        <Input id={`${id}-name`} name="name" required maxLength={200} />
      </Field>
      <Field label="Data region" htmlFor={`${id}-region`}>
        <NativeSelect
          id={`${id}-region`}
          name="region"
          required
          defaultValue="local"
          className="font-mono"
        >
          <option value="local">local</option>
          <option value="us-east">us-east</option>
          <option value="us-west">us-west</option>
          <option value="eu-west">eu-west</option>
          <option value="ap-south">ap-south</option>
        </NativeSelect>
      </Field>
      <ApiFailureNotice failure={failure} />
      <Button type="submit" disabled={pending} className="justify-self-start">
        {pending ? "Creating…" : submitLabel}
      </Button>
    </form>
  );
}

// --- First-run guide ----------------------------------------------------------

function FirstRunGuide({
  teams,
  progress,
  onProgress,
  onOpenProject,
  onCompleted,
}: {
  readonly teams: readonly Team[];
  readonly progress: OnboardingProgress;
  readonly onProgress: (next: OnboardingProgress) => void;
  readonly onOpenProject: (projectId: string) => void;
  readonly onCompleted: () => void;
}) {
  return (
    <Card aria-labelledby="guide-title">
      <CardHeader>
        <Eyebrow>First run</Eyebrow>
        <CardTitle id="guide-title">Connect your first project</CardTitle>
        <CardAction>
          <Button
            variant="outline"
            size="sm"
            onClick={() => onProgress({ ...progress, dismissed: true })}
          >
            Dismiss
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4">
        <div className="grid gap-3">
          <h3 className="text-base">Create a project</h3>
          <p className="m-0 text-sm text-muted-foreground">
            Choose where the project lives and the region that holds its data. Nothing is created
            until you submit. Once it exists, open the project for its keys, API URL, quickstart
            snippet, and a connection check.
          </p>
          <ProjectCreateForm
            teams={teams}
            submitLabel="Create and provision"
            onCreated={(project) => {
              onCompleted();
              onOpenProject(project.id);
            }}
          />
        </div>
      </CardContent>
    </Card>
  );
}

// --- Onboarding progress ------------------------------------------------------

const ONBOARDING_STEPS = ["create", "provision", "keys", "check", "done"] as const;
type OnboardingStep = (typeof ONBOARDING_STEPS)[number];

interface OnboardingProgress {
  readonly version: 1;
  readonly step: OnboardingStep;
  readonly projectId?: string;
  readonly dismissed: boolean;
}

const FRESH_PROGRESS: OnboardingProgress = { version: 1, step: "create", dismissed: false };

function progressStorageKey(developerId: string): string {
  return `mako-console:onboarding:${developerId}`;
}

/** Reads stored progress, trusting nothing that does not match the shape. */
function readProgress(developerId: string): OnboardingProgress {
  if (developerId === "") {
    return FRESH_PROGRESS;
  }
  try {
    const raw = window.sessionStorage.getItem(progressStorageKey(developerId));
    if (raw === null) {
      return FRESH_PROGRESS;
    }
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) {
      return FRESH_PROGRESS;
    }
    const record = parsed as Record<string, unknown>;
    const step = record.step;
    if (record.version !== 1 || !isOnboardingStep(step)) {
      return FRESH_PROGRESS;
    }
    const projectId = record.projectId;
    return {
      version: 1,
      step,
      ...(typeof projectId === "string" && /^[A-Za-z0-9_-]{1,128}$/u.test(projectId)
        ? { projectId }
        : {}),
      dismissed: record.dismissed === true,
    };
  } catch {
    return FRESH_PROGRESS;
  }
}

function isOnboardingStep(value: unknown): value is OnboardingStep {
  return typeof value === "string" && (ONBOARDING_STEPS as readonly string[]).includes(value);
}

function useOnboardingProgress(
  developerId: string,
): [OnboardingProgress, (next: OnboardingProgress) => void] {
  const [progress, setState] = useState(() => readProgress(developerId));
  useEffect(() => {
    setState(readProgress(developerId));
  }, [developerId]);
  const update = useCallback(
    (next: OnboardingProgress) => {
      setState(next);
      if (developerId === "") {
        return;
      }
      try {
        window.sessionStorage.setItem(progressStorageKey(developerId), JSON.stringify(next));
      } catch {
        // Storage may be unavailable; progress then lives for this page only.
      }
    },
    [developerId],
  );
  return [progress, update];
}

// --- Summary loading ----------------------------------------------------------

interface UsageSummary {
  readonly environment: Environment | null;
  readonly storageBytes: number | null;
  readonly replicationBytes: number | null;
}

interface SummaryLoader {
  environment(projectId: string): Promise<Environment | null>;
  plan(teamId: string): Promise<string>;
  usage(projectId: string): Promise<UsageSummary>;
}

/**
 * Per-project summaries behind one bounded queue. Each summary is requested once
 * per page (a team's plan is shared by all of its projects); a failed request is
 * forgotten so a later mount can try again.
 */
function createSummaryLoader(client: MakoManagementClient, limit = MAX_IN_FLIGHT): SummaryLoader {
  const queue = createBoundedQueue(limit);
  const plans = new Map<string, Promise<string>>();
  const environments = new Map<string, Promise<Environment | null>>();
  const usage = new Map<string, Promise<UsageSummary>>();
  // Not queued itself: it runs inside whichever queued task needs it first, so
  // a full queue can never wait on a task that is itself waiting for the queue.
  const activeEnvironment = (projectId: string) =>
    memoize(environments, projectId, async () => {
      const list = await client.listEnvironments(projectId);
      return list.find((environment) => environment.state === "active") ?? null;
    });
  return {
    environment: (projectId) => queue.run(() => activeEnvironment(projectId)),
    plan: (teamId) =>
      memoize(plans, teamId, () =>
        queue.run(async () => (await client.getTeamBill(teamId)).planId),
      ),
    usage: (projectId) =>
      memoize(usage, projectId, () =>
        queue.run(async () => {
          const environment = await activeEnvironment(projectId);
          if (environment === null) {
            return { environment: null, storageBytes: null, replicationBytes: null };
          }
          const page = await client.queryProjectUsage(projectId, environment.id, {
            limit: USAGE_SAMPLE_LIMIT,
          });
          return summarizeUsage(environment, page.items);
        }),
      ),
  };
}

/**
 * Storage is a level: the newest sample is the headline. Replication bytes are
 * a flow over the period: the samples in the window add up.
 */
function summarizeUsage(
  environment: Environment,
  records: readonly ObservabilityRecord[],
): UsageSummary {
  let storage: { readonly timestamp: number; readonly quantity: number } | null = null;
  let replication: number | null = null;
  for (const record of records) {
    if (record.payload.kind !== "usage") {
      continue;
    }
    if (record.payload.resource === "storage_bytes") {
      const timestamp = Date.parse(record.timestamp);
      if (storage === null || timestamp >= storage.timestamp) {
        storage = { timestamp, quantity: record.payload.quantity };
      }
    } else if (record.payload.resource === "replication_bytes_per_month") {
      replication = (replication ?? 0) + record.payload.quantity;
    }
  }
  return {
    environment,
    storageBytes: storage === null ? null : storage.quantity,
    replicationBytes: replication,
  };
}

function memoize<T>(cache: Map<string, Promise<T>>, key: string, create: () => Promise<T>) {
  const existing = cache.get(key);
  if (existing !== undefined) {
    return existing;
  }
  const created = create();
  cache.set(key, created);
  created.catch(() => cache.delete(key));
  return created;
}

function createBoundedQueue(limit: number) {
  let inFlight = 0;
  const waiting: Array<() => void> = [];
  return {
    async run<T>(task: () => Promise<T>): Promise<T> {
      if (inFlight >= limit) {
        await new Promise<void>((resolve) => waiting.push(resolve));
      }
      inFlight += 1;
      try {
        return await task();
      } finally {
        inFlight -= 1;
        waiting.shift()?.();
      }
    },
  };
}

// --- Helpers ------------------------------------------------------------------

function sortOwners(teams: readonly Team[]): Team[] {
  return [...teams].sort((left, right) => {
    if (left.kind !== right.kind) {
      return left.kind === "personal" ? -1 : 1;
    }
    return 0;
  });
}

function formatBytes(value: number): string {
  if (value >= 1024 ** 3) {
    return `${(value / 1024 ** 3).toFixed(2)} GiB`;
  }
  if (value >= 1024 ** 2) {
    return `${(value / 1024 ** 2).toFixed(1)} MiB`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${value} B`;
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
