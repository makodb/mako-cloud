import type { Environment, ObservabilityPage, Project } from "@mako-cloud/management-sdk";
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
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
import { ChevronRight } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useId, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction } from "./safety.js";

/** A disclosure's summary: a quiet row that turns its chevron when open. */
const SUMMARY =
  "flex cursor-pointer list-none items-center gap-2 px-4 py-2.5 text-sm font-medium text-muted-foreground select-none hover:text-foreground [&::-webkit-details-marker]:hidden";

export function ProjectsPanel({
  teamId,
  scope = "team",
  onOpen,
}: {
  readonly teamId: string;
  /**
   * A personal space owns its projects implicitly: creation posts without a
   * `teamId` and the server resolves the caller's space.
   */
  readonly scope?: "team" | "personal";
  readonly onOpen: (projectId: string) => void;
}) {
  const client = useManagementClient();
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setProjects(await client.listProjects(teamId));
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, teamId]);
  useEffect(() => {
    void reload();
  }, [reload]);
  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    try {
      const project = await client.createProject(
        { ...(scope === "personal" ? {} : { teamId }), ...projectInput(form) },
        idempotencyKey(),
      );
      form.reset();
      onOpen(project.id);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  return (
    <Card className="lg:col-span-2" aria-labelledby="projects-title">
      <CardHeader>
        <CardTitle id="projects-title">Projects</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {projects === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading projects…</p>
        ) : projects.length === 0 ? null : (
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Project</TableHead>
                <TableHead scope="col">Region</TableHead>
                <TableHead scope="col">State</TableHead>
                <TableHead scope="col" className="text-right">
                  <span className="sr-only">Open</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {projects.map((project) => (
                <TableRow key={project.id}>
                  <TableCell className="font-medium">{project.name}</TableCell>
                  <TableCell className="font-mono text-xs text-muted-foreground">
                    {project.region}
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge state={project.state} />
                  </TableCell>
                  <TableCell className="text-right">
                    <Button variant="outline" size="sm" onClick={() => onOpen(project.id)}>
                      Open <span className="sr-only">{project.name}</span>
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
        <details className="group rounded-lg border">
          <summary className={SUMMARY}>
            <ChevronRight
              aria-hidden="true"
              className="size-4 shrink-0 transition-transform group-open:rotate-90"
            />
            Create project
          </summary>
          <form
            className="grid max-w-md gap-4 border-t px-4 py-4"
            onSubmit={(event) => void create(event)}
          >
            <ProjectFormFields />
            <Button type="submit" className="justify-self-start">
              Create and provision
            </Button>
          </form>
        </details>
      </CardContent>
    </Card>
  );
}

/**
 * The empty state of a developer without a personal space yet. The first
 * individual project is created without naming a team; the server creates the
 * personal space that owns it on the way, so the caller reloads its teams.
 */
export function FirstProjectPanel({ onCreated }: { readonly onCreated: () => Promise<void> }) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    setPending(true);
    setFailure(null);
    try {
      await client.createProject(projectInput(form), idempotencyKey());
      form.reset();
      await onCreated();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <Card aria-labelledby="first-project-title">
      <CardHeader>
        <CardTitle id="first-project-title">Create your first project</CardTitle>
        <CardDescription>
          Create a project owned by your account. You can transfer it to a team later.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        <form className="grid max-w-md gap-4" onSubmit={(event) => void create(event)}>
          <ProjectFormFields />
          <Button type="submit" disabled={pending} className="justify-self-start">
            {pending ? "Creating…" : "Create and provision"}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}

function ProjectFormFields() {
  const id = useId();
  return (
    <>
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
    </>
  );
}

function projectInput(form: HTMLFormElement): { readonly name: string; readonly region: string } {
  const data = new FormData(form);
  return {
    name: String(data.get("name") ?? "").trim(),
    region: String(data.get("region") ?? "").trim(),
  };
}

export function ProjectScreen({
  projectId,
  onBack,
  onOpenCollections,
  onOpenWorkspace,
  onOpenFunctions,
  onOpenObservability,
  onOpenUsers,
  onOpenSecurity,
}: {
  readonly projectId: string;
  readonly onBack: (teamId: string) => void;
  readonly onOpenCollections: (environmentId: string) => void;
  readonly onOpenWorkspace: (environmentId: string) => void;
  readonly onOpenFunctions: (environmentId: string) => void;
  readonly onOpenObservability: (environmentId: string) => void;
  readonly onOpenUsers: (environmentId: string) => void;
  readonly onOpenSecurity: (environmentId: string) => void;
}) {
  const client = useManagementClient();
  const [project, setProject] = useState<Project | null>(null);
  const [environments, setEnvironments] = useState<Environment[] | null>(null);
  const [health, setHealth] = useState<ObservabilityPage | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      const [nextProject, nextEnvironments] = await Promise.all([
        client.getProject(projectId),
        client.listEnvironments(projectId),
      ]);
      setProject(nextProject);
      setEnvironments(nextEnvironments);
      const firstEnvironment = nextEnvironments[0];
      setHealth(
        firstEnvironment === undefined
          ? null
          : await client.queryProjectHealth(projectId, firstEnvironment.id, { limit: 20 }),
      );
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);
  useEffect(() => {
    if (project?.state !== "provisioning") {
      return;
    }
    const timer = window.setInterval(() => void reload(), 5_000);
    return () => window.clearInterval(timer);
  }, [project?.state, reload]);

  const projectAction = async (action: "suspend" | "restore" | "delete") => {
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
      await reload();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  const environmentNameId = useId();

  return (
    <section aria-labelledby="project-title" className="grid gap-6">
      <Button
        variant="ghost"
        size="sm"
        className="justify-self-start text-muted-foreground"
        disabled={project === null}
        onClick={() => project !== null && onBack(project.teamId)}
      >
        ← Back
      </Button>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <Eyebrow>Project</Eyebrow>
          <h1 id="project-title" className="text-2xl">
            {project?.name ?? "Loading…"}
          </h1>
        </div>
        {project === null ? null : <LifecycleBadge state={project.state} />}
      </div>
      <ApiFailureNotice failure={failure} />
      {project === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading project…</p>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          <LifecyclePanel
            project={project}
            onSuspend={() => void projectAction("suspend")}
            onRestore={() => void projectAction("restore")}
            onDelete={() => void projectAction("delete")}
          />
          <HealthPanel page={health} />
          <Card className="lg:col-span-2" aria-labelledby="environments-title">
            <CardHeader>
              <CardTitle id="environments-title">Environments</CardTitle>
            </CardHeader>
            <CardContent className="grid gap-4">
              <div className="grid gap-3">
                {environments?.map((environment) => (
                  <EnvironmentRow
                    key={environment.id}
                    environment={environment}
                    onChanged={reload}
                    onOpenCollections={() => onOpenCollections(environment.id)}
                    onOpenWorkspace={() => onOpenWorkspace(environment.id)}
                    onOpenFunctions={() => onOpenFunctions(environment.id)}
                    onOpenObservability={() => onOpenObservability(environment.id)}
                    onOpenUsers={() => onOpenUsers(environment.id)}
                    onOpenSecurity={() => onOpenSecurity(environment.id)}
                  />
                ))}
              </div>
              <details className="group rounded-lg border">
                <summary className={SUMMARY}>
                  <ChevronRight
                    aria-hidden="true"
                    className="size-4 shrink-0 transition-transform group-open:rotate-90"
                  />
                  Create environment
                </summary>
                <form
                  className="grid max-w-md gap-4 border-t px-4 py-4"
                  onSubmit={(event) => void createEnvironment(event)}
                >
                  <Field label="Environment name" htmlFor={environmentNameId}>
                    <Input id={environmentNameId} name="name" required maxLength={100} />
                  </Field>
                  <Button type="submit" className="justify-self-start">
                    Create environment
                  </Button>
                </form>
              </details>
            </CardContent>
          </Card>
        </div>
      )}
    </section>
  );
}

function LifecyclePanel({
  project,
  onSuspend,
  onRestore,
  onDelete,
}: {
  readonly project: Project;
  readonly onSuspend: () => void;
  readonly onRestore: () => void;
  readonly onDelete: () => void;
}) {
  return (
    <Card aria-labelledby="lifecycle-title">
      <CardHeader>
        <CardTitle id="lifecycle-title">Provisioning and lifecycle</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-3 text-sm">
        <p className="m-0">
          Current state: <strong>{project.state.replaceAll("_", " ")}</strong>
        </p>
        {project.failureDiagnostic === undefined ? null : (
          <p role="alert" className="m-0 text-destructive">
            {project.failureDiagnostic}
          </p>
        )}
        {project.deletionDeadline === undefined ? null : (
          <p className="m-0 text-muted-foreground">
            Restorable until {new Date(project.deletionDeadline).toLocaleString()}.
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          <Button onClick={onSuspend} disabled={project.state !== "active"}>
            Suspend
          </Button>
          <Button
            variant="outline"
            onClick={onRestore}
            disabled={!(["suspended", "deletion_grace"] as string[]).includes(project.state)}
          >
            Restore
          </Button>
          <Button
            variant="destructive"
            onClick={onDelete}
            disabled={!(["active", "suspended", "failed"] as string[]).includes(project.state)}
          >
            Request deletion
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function EnvironmentRow({
  environment,
  onChanged,
  onOpenCollections,
  onOpenWorkspace,
  onOpenFunctions,
  onOpenObservability,
  onOpenUsers,
  onOpenSecurity,
}: {
  readonly environment: Environment;
  readonly onChanged: () => Promise<void>;
  readonly onOpenCollections: () => void;
  readonly onOpenWorkspace: () => void;
  readonly onOpenFunctions: () => void;
  readonly onOpenObservability: () => void;
  readonly onOpenUsers: () => void;
  readonly onOpenSecurity: () => void;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const act = async (action: "suspend" | "restore" | "delete") => {
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
      await onChanged();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  const active = environment.state === "active";
  return (
    <Card as="article" className="gap-3 py-4">
      <CardContent className="flex flex-wrap items-center gap-3 px-4">
        <div className="grid min-w-0 gap-0.5">
          <strong className="text-sm font-semibold">{environment.name}</strong>
          <code className="font-mono text-xs text-muted-foreground">{environment.id}</code>
          {environment.deletionDeadline === undefined ? null : (
            <small className="text-xs text-muted-foreground">
              Restorable until {new Date(environment.deletionDeadline).toLocaleString()}
            </small>
          )}
        </div>
        <LifecycleBadge state={environment.state} />
      </CardContent>
      <CardContent className="flex flex-wrap gap-2 px-4">
        <Button size="sm" disabled={!active} onClick={onOpenWorkspace}>
          Workspace
        </Button>
        <Button size="sm" disabled={!active} onClick={onOpenCollections}>
          Collections
        </Button>
        <Button variant="outline" size="sm" disabled={!active} onClick={onOpenFunctions}>
          Functions
        </Button>
        <Button variant="outline" size="sm" disabled={!active} onClick={onOpenObservability}>
          Observability
        </Button>
        <Button variant="outline" size="sm" disabled={!active} onClick={onOpenUsers}>
          Users
        </Button>
        <Button variant="outline" size="sm" disabled={!active} onClick={onOpenSecurity}>
          Credentials
        </Button>
        <Button variant="outline" size="sm" disabled={!active} onClick={() => void act("suspend")}>
          Suspend
        </Button>
        <Button
          variant="outline"
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
      </CardContent>
      {failure === null ? null : (
        <CardContent className="px-4">
          <ApiFailureNotice failure={failure} />
        </CardContent>
      )}
    </Card>
  );
}

function HealthPanel({ page }: { readonly page: ObservabilityPage | null }) {
  const health =
    page?.items.flatMap((record) =>
      record.payload.kind === "health"
        ? [{ timestamp: record.timestamp, payload: record.payload }]
        : [],
    ) ?? [];
  return (
    <Card aria-labelledby="health-title">
      <CardHeader>
        <CardTitle id="health-title">Data-plane health</CardTitle>
      </CardHeader>
      <CardContent className="text-sm">
        {page === null ? (
          <p className="m-0 text-muted-foreground">No environment health is available yet.</p>
        ) : health.length === 0 ? (
          <p className="m-0 text-muted-foreground">No retained health observations.</p>
        ) : (
          <ul className="m-0 grid list-none gap-1.5 p-0">
            {health.map((record) => {
              const payload = record.payload;
              return (
                <li key={`${record.timestamp}-${payload.service}-${payload.region}`}>
                  <strong className="font-medium">{payload.service}</strong> in {payload.region}:{" "}
                  {payload.status}
                </li>
              );
            })}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

/** How a lifecycle state is coloured: green when serving, amber when paused, red when broken or going. */
const LIFECYCLE_VARIANTS: Readonly<
  Record<string, "positive" | "warning" | "destructive" | "secondary" | "outline">
> = {
  active: "positive",
  provisioning: "secondary",
  restoring: "secondary",
  suspended: "warning",
  deletion_grace: "warning",
  failed: "destructive",
  deleting: "destructive",
  deleted: "outline",
};

export function LifecycleBadge({ state }: { readonly state: string }) {
  return (
    <Badge variant={LIFECYCLE_VARIANTS[state] ?? "secondary"} data-state={state}>
      <span className="sr-only">Status: </span>
      {state.replaceAll("_", " ")}
    </Badge>
  );
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}

function deletionConfirmation(name: string | undefined): string {
  return `delete:${name ?? "resource"}`;
}
