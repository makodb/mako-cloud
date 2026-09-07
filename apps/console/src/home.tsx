// Home dashboard and first-run onboarding.
//
// The dashboard is a read-only aggregation of existing management calls:
// `listTeams` → `listProjects` per owner, then — lazily, per card, with a
// bounded number of requests in flight — the owner's plan, the project's
// headline usage, and the developer's recent activity. One project's failing
// summary marks only its own card. The guided first run sequences what the
// environment "Connect" screen already does (create → wait for active → keys →
// connection check) and keeps its progress in browser storage keyed by the
// developer, never storing a credential value.
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Eyebrow,
  Field,
  Input,
  NativeSelect,
  cn,
} from "@mako-cloud/ui";
import { CheckCircle2, ChevronRight, Copy } from "lucide-react";
import {
  type FormEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";

import type {
  ConnectionCheck,
  ConnectMetadata,
  Environment,
  MakoManagementClient,
  ObservabilityRecord,
  Project,
  Team,
} from "@mako-cloud/management-sdk";
import { createMakoRxdbConnectTemplateV1 } from "@mako-cloud/rxdb";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useDeveloperAuth } from "./auth.js";
import { useManagementClient } from "./management.js";
import { FirstProjectPanel, LifecycleBadge } from "./projects.js";
import { OneTimeSecretValue } from "./safety.js";

/** Summary requests in flight at once across every card on the page. */
const MAX_IN_FLIGHT = 4;
/** Usage samples read per project; the newest level and the period's flows come out of them. */
const USAGE_SAMPLE_LIMIT = 50;
/** Audit events read per project for the activity feed. */
const ACTIVITY_LIMIT = 10;
/** Projects whose activity feeds are merged on the home page. */
const ACTIVITY_PROJECTS = 3;
/** Activity rows shown after merging. */
const ACTIVITY_ROWS = 15;
/** Provisioning and environment readiness are polled at this interval during the first run. */
const POLL_MILLISECONDS = 2_000;
/** The client version the connection check is run for, as on the Connect screen. */
const RXDB_VERSION = "17.0.0";
/** The snippet never embeds a credential; the copied key replaces this marker. */
const PUBLIC_KEY_PLACEHOLDER = "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY";

/** A small label over a fact. `dt` keeps the definition list intact, so this is
 * the kit's Eyebrow styling on the term rather than the component. */
const FACT_LABEL = "text-xs font-semibold uppercase tracking-wider text-muted-foreground";

/** A quiet line of text: a loading note, an empty list, a caption. */
const MUTED = "m-0 text-sm text-muted-foreground";

export function HomeDashboard({
  onOpenTeam,
  onOpenProject,
}: {
  readonly onOpenTeam: (teamId: string) => void;
  readonly onOpenProject: (projectId: string) => void;
}) {
  const client = useManagementClient();
  const { state } = useDeveloperAuth();
  const developerId = state.status === "authenticated" ? state.session.profile.id : "";
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
    (teams?.every((team) => groups[team.id]?.status === "ready") ?? false) &&
    readyProjects.length === 0;
  const guideInFlight = progress.step !== "done" && progress.projectId !== undefined;
  const guideActive = !progress.dismissed && (noProjects || guideInFlight);
  const showGuideAgain = progress.dismissed && (noProjects || guideInFlight);
  const activityProjects = readyProjects.slice(0, ACTIVITY_PROJECTS);
  const ownerName = (project: Project): string =>
    teams?.find((team) => team.id === project.teamId)?.kind === "personal"
      ? "Personal space"
      : (teams?.find((team) => team.id === project.teamId)?.name ?? project.teamId);

  const completeGuide = () => {
    setProgress({ ...progress, step: "done", dismissed: false });
    void reload();
  };

  return (
    <section className="grid gap-6" aria-labelledby="home-title" aria-busy={loading}>
      <div className="grid gap-1">
        <Eyebrow>Dashboard</Eyebrow>
        <h1 id="home-title" className="text-2xl">
          Home
        </h1>
      </div>
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
      {teams !== null && !noProjects ? (
        <>
          <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,2fr)_minmax(18rem,1fr)]">
            <div className="grid min-w-0 gap-8">
              {personalSpace === undefined ? (
                <section className="grid gap-4" aria-labelledby="owner-personal-title">
                  <div className="grid gap-1">
                    <Eyebrow>Personal space</Eyebrow>
                    <h2 id="owner-personal-title" className="text-lg">
                      Your projects
                    </h2>
                  </div>
                  <FirstProjectPanel onCreated={reload} />
                </section>
              ) : (
                <OwnerGroup
                  team={personalSpace}
                  load={groups[personalSpace.id] ?? { status: "loading" }}
                  loader={loader}
                  ownerName={ownerName}
                  onOpenProject={onOpenProject}
                  onOpenTeam={onOpenTeam}
                />
              )}
              {joinedTeams.map((team) => (
                <OwnerGroup
                  key={team.id}
                  team={team}
                  load={groups[team.id] ?? { status: "loading" }}
                  loader={loader}
                  ownerName={ownerName}
                  onOpenProject={onOpenProject}
                  onOpenTeam={onOpenTeam}
                />
              ))}
            </div>
            <RecentActivity projects={activityProjects} loader={loader} />
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
              <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
                {joinedTeams.map((team) => (
                  <Button
                    key={team.id}
                    variant="outline"
                    className="resource-card h-auto flex-col items-start gap-1 px-4 py-3 text-left whitespace-normal"
                    onClick={() => onOpenTeam(team.id)}
                  >
                    <strong className="text-sm font-semibold">{team.name}</strong>
                    <span className="text-xs font-normal text-muted-foreground">
                      {team.state.replaceAll("_", " ")}
                    </span>
                  </Button>
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
  );
}

// --- Project groups and cards ------------------------------------------------

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
}: {
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
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="grid gap-1">
          <Eyebrow>{personal ? "Personal space" : "Team"}</Eyebrow>
          <h2 id={headingId} className="text-lg">
            {personal ? "Your projects" : team.name}
          </h2>
        </div>
        {personal ? (
          <Button variant="outline" size="sm" onClick={() => onOpenTeam(team.id)}>
            Billing and plan
          </Button>
        ) : null}
      </div>
      {load.status === "loading" ? (
        <p className={MUTED} aria-live="polite">
          Loading projects…
        </p>
      ) : load.status === "failed" ? (
        <ApiFailureNotice failure={load.failure} />
      ) : load.projects.length === 0 ? (
        <p className={MUTED}>
          {personal ? "No individual projects yet." : "This team has no projects yet."}
        </p>
      ) : (
        <div className="grid gap-4 grid-cols-[repeat(auto-fill,minmax(17rem,1fr))]">
          {load.projects.map((project) => (
            <ProjectCard
              key={project.id}
              project={project}
              owner={ownerName(project)}
              loader={loader}
              onOpen={() => onOpenProject(project.id)}
            />
          ))}
        </div>
      )}
      {personal ? (
        <details className="group rounded-lg border bg-card">
          <summary className="flex cursor-pointer list-none items-center gap-2 px-4 py-2.5 text-sm font-medium text-muted-foreground select-none hover:text-foreground [&::-webkit-details-marker]:hidden">
            <ChevronRight
              aria-hidden="true"
              className="size-4 shrink-0 transition-transform group-open:rotate-90"
            />
            Create project
          </summary>
          <div className="border-t px-4 py-4">
            <ProjectCreateForm
              teams={[]}
              submitLabel="Create and provision"
              onCreated={(project) => onOpenProject(project.id)}
            />
          </div>
        </details>
      ) : null}
    </section>
  );
}

type Summary<T> =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly value: T }
  | { readonly status: "unavailable" };

function ProjectCard({
  project,
  owner,
  loader,
  onOpen,
}: {
  readonly project: Project;
  readonly owner: string;
  readonly loader: SummaryLoader;
  readonly onOpen: () => void;
}) {
  const [plan, setPlan] = useState<Summary<string>>({ status: "loading" });
  const [usage, setUsage] = useState<Summary<UsageSummary>>({ status: "loading" });
  useEffect(() => {
    let active = true;
    setPlan({ status: "loading" });
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
    <Card
      as="article"
      // `home-project-card` is the hook the browser suite selects on.
      className="home-project-card gap-4 py-4 aria-busy:border-dashed"
      aria-labelledby={headingId}
      aria-busy={plan.status === "loading" || usage.status === "loading"}
    >
      <CardHeader className="px-4">
        <CardTitle as="h3" id={headingId} className="leading-snug break-words">
          {project.name}
        </CardTitle>
        <CardAction>
          <LifecycleBadge state={project.state} />
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4 px-4">
        <dl className="m-0 grid grid-cols-2 gap-x-4 gap-y-3 text-sm">
          <Fact label="Owner">{owner}</Fact>
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
        <Button variant="outline" size="sm" className="justify-self-start" onClick={onOpen}>
          Open <span className="sr-only">{project.name}</span>
        </Button>
      </CardContent>
    </Card>
  );
}

/** One fact on a card: its name over its value. */
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

// --- Recent activity ---------------------------------------------------------

interface ActivityEntry {
  readonly timestamp: string;
  readonly actorId: string;
  readonly action: string;
  readonly target: string;
  readonly outcome: string;
}

type ActivitySource =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly entries: readonly ActivityEntry[] }
  | { readonly status: "unavailable" };

function RecentActivity({
  projects,
  loader,
}: {
  readonly projects: readonly Project[];
  readonly loader: SummaryLoader;
}) {
  const [sources, setSources] = useState<Record<string, ActivitySource>>({});
  const projectIds = projects.map((project) => project.id).join(",");
  useEffect(() => {
    let active = true;
    const ids = projectIds === "" ? [] : projectIds.split(",");
    setSources(Object.fromEntries(ids.map((id) => [id, { status: "loading" as const }])));
    for (const id of ids) {
      void loader.activity(id).then(
        (entries) =>
          active && setSources((current) => ({ ...current, [id]: { status: "ready", entries } })),
        () => active && setSources((current) => ({ ...current, [id]: { status: "unavailable" } })),
      );
    }
    return () => {
      active = false;
    };
  }, [loader, projectIds]);

  const pending = projects.some(
    (project) => (sources[project.id]?.status ?? "loading") === "loading",
  );
  const rows = projects
    .flatMap((project) => {
      const source = sources[project.id];
      return source?.status === "ready"
        ? source.entries.map((entry) => ({ ...entry, project }))
        : [];
    })
    .sort((left, right) => Date.parse(right.timestamp) - Date.parse(left.timestamp))
    .slice(0, ACTIVITY_ROWS);
  const unavailable = projects.filter((project) => sources[project.id]?.status === "unavailable");

  // The kit's Card renders a section, an article, or a div; a feed beside the
  // main content is a complementary landmark, so the card is drawn by hand.
  return (
    <aside
      className="flex min-w-0 flex-col gap-4 rounded-xl border bg-card py-4 text-card-foreground shadow-xs"
      aria-labelledby="activity-title"
      aria-busy={pending}
    >
      <CardHeader className="px-4">
        <CardTitle id="activity-title">Recent activity</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-3 px-4 text-sm">
        {projects.length === 0 ? <p className={MUTED}>No projects to report on yet.</p> : null}
        {unavailable.map((project) => (
          <p key={project.id} className="m-0 text-sm text-destructive" role="status">
            Activity unavailable for {project.name}
          </p>
        ))}
        {pending ? (
          <p className={MUTED} aria-live="polite">
            Loading activity…
          </p>
        ) : null}
        {!pending && rows.length === 0 && unavailable.length < projects.length ? (
          <p className={MUTED}>No recent audited actions.</p>
        ) : null}
        {rows.length === 0 ? null : (
          <ol className="m-0 grid list-none gap-3 p-0">
            {rows.map((row) => (
              <li
                key={`${row.project.id}-${row.timestamp}-${row.action}-${row.target}`}
                className="grid gap-0.5 border-b pb-3 last:border-b-0 last:pb-0"
              >
                <span className="break-words">
                  <strong className="font-mono text-xs font-medium">{row.actorId}</strong>{" "}
                  {row.action.replaceAll("_", " ")}{" "}
                  <code className="font-mono text-xs">{row.target}</code>
                  {row.outcome === "allowed" ? null : (
                    <span className="font-medium text-destructive"> ({row.outcome})</span>
                  )}
                </span>
                <small className="text-xs text-muted-foreground">
                  {row.project.name} ·{" "}
                  <time dateTime={row.timestamp}>{new Date(row.timestamp).toLocaleString()}</time>
                </small>
              </li>
            ))}
          </ol>
        )}
      </CardContent>
    </aside>
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
            <option value="">Personal space</option>
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
        <Input
          id={`${id}-region`}
          name="region"
          required
          maxLength={64}
          placeholder="us-east"
          className="font-mono"
        />
      </Field>
      <ApiFailureNotice failure={failure} />
      <Button type="submit" disabled={pending} className="justify-self-start">
        {pending ? "Creating…" : submitLabel}
      </Button>
    </form>
  );
}

// --- First-run guide ----------------------------------------------------------

const GUIDE_STEPS = [
  { id: "create", label: "Create a project" },
  { id: "provision", label: "Wait for provisioning" },
  { id: "keys", label: "Copy your keys" },
  { id: "check", label: "Check the connection" },
] as const;

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
  const client = useManagementClient();
  const stepIndex = GUIDE_STEPS.findIndex((step) => step.id === progress.step);
  const projectId = progress.projectId;
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
      <CardContent className="grid gap-5">
        <ol
          className="m-0 grid list-none gap-2 p-0 sm:grid-cols-2 lg:grid-cols-4"
          aria-label="First-run steps"
        >
          {GUIDE_STEPS.map((step, index) => {
            const done = index < stepIndex;
            const current = index === stepIndex;
            return (
              <li
                key={step.id}
                className={cn(
                  "flex items-center gap-2.5 rounded-lg border px-3 py-2 text-sm font-medium text-muted-foreground",
                  current && "border-primary/50 bg-accent text-foreground",
                  done && "text-foreground",
                )}
                aria-current={current ? "step" : undefined}
              >
                <span
                  className={cn(
                    "inline-grid size-6 shrink-0 place-items-center rounded-full bg-muted text-xs font-semibold tabular-nums",
                    (current || done) && "bg-primary text-primary-foreground",
                  )}
                  aria-hidden="true"
                >
                  {index + 1}
                </span>
                {step.label}
              </li>
            );
          })}
        </ol>
        {progress.step === "create" || projectId === undefined ? (
          <div className="grid gap-3">
            <h3 className="text-base">Create a project</h3>
            <p className="m-0 text-sm text-muted-foreground">
              Choose where the project lives and the region that holds its data. Nothing is created
              until you submit.
            </p>
            <ProjectCreateForm
              teams={teams}
              submitLabel="Create and provision"
              onCreated={(project) =>
                onProgress({ ...progress, step: "provision", projectId: project.id })
              }
            />
          </div>
        ) : progress.step === "provision" ? (
          <ProvisionStep
            client={client}
            projectId={projectId}
            onActive={() => onProgress({ ...progress, step: "keys" })}
            onRestart={() => onProgress({ ...progress, step: "create" })}
          />
        ) : progress.step === "keys" ? (
          <KeysStep
            client={client}
            projectId={projectId}
            onContinue={() => onProgress({ ...progress, step: "check" })}
          />
        ) : (
          <CheckStep
            client={client}
            projectId={projectId}
            onBack={() => onProgress({ ...progress, step: "keys" })}
            onOpenProject={() => {
              onCompleted();
              onOpenProject(projectId);
            }}
            onCompleted={onCompleted}
          />
        )}
      </CardContent>
    </Card>
  );
}

function ProvisionStep({
  client,
  projectId,
  onActive,
  onRestart,
}: {
  readonly client: MakoManagementClient;
  readonly projectId: string;
  readonly onActive: () => void;
  readonly onRestart: () => void;
}) {
  const [project, setProject] = useState<Project | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  // The latest callback is read from a ref so a parent re-render never restarts
  // the poll; the poll itself is keyed only by what it fetches.
  const activeCallback = useRef(onActive);
  activeCallback.current = onActive;
  const settled = project?.state === "active" || project?.state === "failed";
  useEffect(() => {
    if (settled) {
      return;
    }
    let active = true;
    const poll = async () => {
      try {
        const next = await client.getProject(projectId);
        if (!active) {
          return;
        }
        setProject(next);
        setFailure(null);
        if (next.state === "active") {
          active = false;
          activeCallback.current();
        }
      } catch (error) {
        if (active) {
          setFailure(toConsoleApiFailure(error));
        }
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), POLL_MILLISECONDS);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [client, projectId, settled]);
  return (
    <div className="grid gap-3" aria-busy={!settled}>
      <h3 className="text-base">Provisioning {project?.name ?? "your project"}</h3>
      <ApiFailureNotice failure={failure} />
      {project === null ? (
        <p className={MUTED} aria-live="polite">
          Checking the project…
        </p>
      ) : (
        <p className="m-0 flex flex-wrap items-center gap-2 text-sm" aria-live="polite">
          <LifecycleBadge state={project.state} />{" "}
          {project.state === "failed"
            ? "Provisioning failed."
            : project.state === "active"
              ? "The project is active."
              : "The data plane is being prepared. This page checks again every few seconds."}
        </p>
      )}
      {project?.state === "failed" ? (
        <>
          {project.failureDiagnostic === undefined ? null : (
            <p role="alert" className="m-0 text-sm text-destructive">
              {project.failureDiagnostic}
            </p>
          )}
          <Button variant="outline" className="justify-self-start" onClick={onRestart}>
            Start over with a new project
          </Button>
        </>
      ) : null}
    </div>
  );
}

function KeysStep({
  client,
  projectId,
  onContinue,
}: {
  readonly client: MakoManagementClient;
  readonly projectId: string;
  readonly onContinue: () => void;
}) {
  const [environment, setEnvironment] = useState<Environment | null>(null);
  const [environmentState, setEnvironmentState] = useState<string | null>(null);
  const [metadata, setMetadata] = useState<ConnectMetadata | null>(null);
  const [metadataFailure, setMetadataFailure] = useState<ConsoleApiFailure | null>(null);
  const [issued, setIssued] = useState<{ readonly id: string; readonly value: string } | null>(
    null,
  );
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);

  // The first environment is created with the project and may still be
  // provisioning when the project itself is active; wait for it here.
  useEffect(() => {
    if (environment !== null) {
      return;
    }
    let active = true;
    const poll = async () => {
      try {
        const environments = await client.listEnvironments(projectId);
        const ready = environments.find((item) => item.state === "active");
        if (!active) {
          return;
        }
        if (ready !== undefined) {
          setEnvironment(ready);
        } else {
          setEnvironmentState(environments[0]?.state ?? "provisioning");
        }
        setFailure(null);
      } catch (error) {
        if (active) {
          setFailure(toConsoleApiFailure(error));
        }
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), POLL_MILLISECONDS);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [client, environment, projectId]);
  useEffect(() => {
    if (environment === null) {
      return;
    }
    let active = true;
    void client.getConnectMetadata(projectId, environment.id).then(
      (value) => {
        if (active) {
          setMetadata(value);
          setMetadataFailure(null);
        }
      },
      (error: unknown) => active && setMetadataFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
    };
  }, [client, environment, projectId]);

  const issueKey = async () => {
    if (environment === null) {
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      const issue = await client.createPublicProjectKey(
        projectId,
        environment.id,
        publicKeyCredentialId(),
        idempotencyKey(),
      );
      setIssued({ id: issue.credential.id, value: issue.value });
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setPending(false);
    }
  };
  const snippet =
    environment === null || metadata === null
      ? ""
      : createMakoRxdbConnectTemplateV1({
          endpoint: metadata.publicEndpoint,
          projectId,
          environmentId: environment.id,
          collectionId: "todos",
          schemaVersion: 1,
          publicProjectKey: PUBLIC_KEY_PLACEHOLDER,
        });

  return (
    <div className="grid gap-3" aria-busy={environment === null}>
      <h3 className="text-base">Your API URL and public key</h3>
      <p className="m-0 text-sm text-muted-foreground">
        Only the public project key belongs in browser or mobile code. It is shown once; store it in
        your application's secret store, then paste it into the snippet.
      </p>
      <ApiFailureNotice failure={failure} />
      {environment === null ? (
        <p className={MUTED} aria-live="polite">
          Waiting for the environment to become active
          {environmentState === null ? "" : ` (currently ${environmentState.replaceAll("_", " ")})`}
          …
        </p>
      ) : (
        <>
          <dl className="m-0 grid gap-3 text-sm sm:grid-cols-3">
            <KeyFact label="Environment">
              {environment.name}{" "}
              <code className="font-mono text-xs text-muted-foreground">{environment.id}</code>
            </KeyFact>
            <KeyFact label="API URL">
              {metadata === null ? (
                metadataFailure === null ? (
                  <span className="text-muted-foreground italic">Loading…</span>
                ) : (
                  <span className="text-destructive">Unavailable: {metadataFailure.message}</span>
                )
              ) : (
                <code className="font-mono text-xs">{metadata.publicEndpoint}</code>
              )}
            </KeyFact>
            <KeyFact label="Public key ID">
              {issued === null ? (
                <span className="text-muted-foreground">Not issued yet</span>
              ) : (
                <code className="font-mono text-xs">{issued.id}</code>
              )}
            </KeyFact>
          </dl>
          {issued === null ? (
            <Button
              className="justify-self-start"
              onClick={() => void issueKey()}
              disabled={pending}
            >
              {pending ? "Issuing…" : "Issue public key"}
            </Button>
          ) : issued.value === "" ? (
            <p className={MUTED}>
              Public key <code className="font-mono text-xs">{issued.id}</code> was issued and its
              value shown once. Rotate it from the Credentials page if you lost it.
            </p>
          ) : (
            <OneTimeSecretValue
              label="public project key"
              value={issued.value}
              onDismiss={() => setIssued({ id: issued.id, value: "" })}
            />
          )}
          {snippet === "" ? null : (
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
              <pre className="m-0 overflow-x-auto p-4 pr-28 font-mono text-xs leading-relaxed">
                {snippet}
              </pre>
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            <Button onClick={onContinue}>Continue to connection check</Button>
          </div>
        </>
      )}
    </div>
  );
}

/** One fact of the connection: its name over its value, on a quiet tile. */
function KeyFact({ label, children }: { readonly label: string; readonly children: ReactNode }) {
  return (
    <div className="grid min-w-0 content-start gap-1 rounded-lg border bg-muted/30 px-3 py-2">
      <dt className={FACT_LABEL}>{label}</dt>
      <dd className="m-0 break-all">{children}</dd>
    </div>
  );
}

function CheckStep({
  client,
  projectId,
  onBack,
  onOpenProject,
  onCompleted,
}: {
  readonly client: MakoManagementClient;
  readonly projectId: string;
  readonly onBack: () => void;
  readonly onOpenProject: () => void;
  readonly onCompleted: () => void;
}) {
  const [check, setCheck] = useState<ConnectionCheck | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  const run = async () => {
    setPending(true);
    setFailure(null);
    try {
      const environments = await client.listEnvironments(projectId);
      const environment = environments.find((item) => item.state === "active");
      if (environment === undefined) {
        setFailure({ message: "The project has no active environment yet.", requestId: null });
        return;
      }
      let publicKeyId: string | undefined;
      try {
        publicKeyId = (await client.getConnectMetadata(projectId, environment.id)).publicKeyId;
      } catch {
        publicKeyId = undefined;
      }
      setCheck(
        await client.checkConnection(projectId, environment.id, {
          ...(publicKeyId === undefined ? {} : { publicKeyId }),
          rxdbVersion: RXDB_VERSION,
        }),
      );
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setPending(false);
    }
  };
  const passed = check?.steps.every((step) => step.state !== "failed") ?? false;
  return (
    <div className="grid gap-3" aria-busy={pending}>
      <h3 className="text-base">Connection check</h3>
      <p className="m-0 text-sm text-muted-foreground">
        Checks DNS, TLS, public routing, readiness, key metadata, and replication routes without
        reading documents or creating a user session.
      </p>
      <ApiFailureNotice failure={failure} />
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => void run()} disabled={pending}>
          {pending ? "Checking…" : check === null ? "Run connection check" : "Run again"}
        </Button>
        <Button variant="outline" onClick={onBack}>
          Back to keys
        </Button>
      </div>
      {check === null ? null : (
        <ol className="m-0 grid list-none gap-2 p-0">
          {check.steps.map((step) => (
            <li key={step.id} className="flex items-start gap-3 text-sm">
              <Badge
                variant={
                  step.state === "passed"
                    ? "positive"
                    : step.state === "failed"
                      ? "destructive"
                      : "secondary"
                }
                className={cn(
                  "status-badge mt-0.5",
                  step.state === "passed" && "success",
                  step.state === "failed" && "error",
                )}
              >
                {humanize(step.state)}
              </Badge>
              <span className="grid gap-0.5">
                <strong className="font-medium">{humanize(step.id)}</strong>
                {step.remediationCode === null || step.remediationCode === undefined ? null : (
                  <small className="text-xs text-muted-foreground">
                    {humanize(step.remediationCode)} ·{" "}
                    {step.retryable ? "retryable" : "configuration change required"}
                  </small>
                )}
              </span>
            </li>
          ))}
        </ol>
      )}
      {check !== null && passed ? (
        <Alert variant="positive" role="status">
          <CheckCircle2 aria-hidden="true" />
          <AlertTitle>Connected.</AlertTitle>
          <AlertDescription className="block">
            <p className="m-0">
              Your project is reachable and ready for an RxDB client. Checked at{" "}
              <time dateTime={new Date(check.checkedAtUnixSeconds * 1000).toISOString()}>
                {new Date(check.checkedAtUnixSeconds * 1000).toLocaleString()}
              </time>
              .
            </p>
            <div className="mt-3 flex flex-wrap gap-2">
              <Button size="sm" onClick={onOpenProject}>
                Open project
              </Button>
              <Button variant="outline" size="sm" onClick={onCompleted}>
                Go to the dashboard
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      ) : null}
    </div>
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
  plan(teamId: string): Promise<string>;
  usage(projectId: string): Promise<UsageSummary>;
  activity(projectId: string): Promise<readonly ActivityEntry[]>;
}

/**
 * Per-card summaries behind one bounded queue. Each summary is requested once
 * per page (a team's plan is shared by all of its cards); a failed request is
 * forgotten so a later mount can try again.
 */
function createSummaryLoader(client: MakoManagementClient, limit = MAX_IN_FLIGHT): SummaryLoader {
  const queue = createBoundedQueue(limit);
  const plans = new Map<string, Promise<string>>();
  const environments = new Map<string, Promise<Environment | null>>();
  const usage = new Map<string, Promise<UsageSummary>>();
  const activity = new Map<string, Promise<readonly ActivityEntry[]>>();
  // Not queued itself: it runs inside whichever queued task needs it first, so
  // a full queue can never wait on a task that is itself waiting for the queue.
  const activeEnvironment = (projectId: string) =>
    memoize(environments, projectId, async () => {
      const list = await client.listEnvironments(projectId);
      return list.find((environment) => environment.state === "active") ?? null;
    });
  return {
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
    activity: (projectId) =>
      memoize(activity, projectId, () =>
        queue.run(async () => {
          const environment = await activeEnvironment(projectId);
          if (environment === null) {
            return [];
          }
          const page = await client.queryAuditEvents(projectId, environment.id, {
            limit: ACTIVITY_LIMIT,
          });
          return page.items.flatMap((record) =>
            record.payload.kind === "audit"
              ? [
                  {
                    timestamp: record.timestamp,
                    actorId: record.payload.actorId,
                    action: record.payload.action,
                    target: record.payload.target,
                    outcome: record.payload.outcome,
                  },
                ]
              : [],
          );
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

function humanize(value: string): string {
  const spaced = value.replace(/([a-z])([A-Z])/gu, "$1 $2").replaceAll("_", " ");
  return `${spaced.charAt(0).toUpperCase()}${spaced.slice(1)}`;
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}

function publicKeyCredentialId(): string {
  return `pk_${globalThis.crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;
}
