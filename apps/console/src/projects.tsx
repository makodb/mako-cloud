import { type FormEvent, useCallback, useEffect, useState } from "react";

import type { Environment, ObservabilityPage, Project } from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction } from "./safety.js";

export function ProjectsPanel({
  teamId,
  onOpen,
}: {
  readonly teamId: string;
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
    const data = new FormData(form);
    try {
      const project = await client.createProject(
        {
          teamId,
          name: String(data.get("name") ?? "").trim(),
          region: String(data.get("region") ?? "").trim(),
        },
        idempotencyKey(),
      );
      form.reset();
      onOpen(project.id);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  return (
    <section className="panel full-span" aria-labelledby="projects-title">
      <h2 id="projects-title">Projects</h2>
      <ApiFailureNotice failure={failure} />
      {projects === null ? (
        <p>Loading projects…</p>
      ) : (
        <div className="resource-list">
          {projects.map((project) => (
            <button
              type="button"
              className="resource-row"
              key={project.id}
              onClick={() => onOpen(project.id)}
            >
              <span>
                <strong>{project.name}</strong>
                <small>{project.region}</small>
              </span>
              <LifecycleBadge state={project.state} />
            </button>
          ))}
        </div>
      )}
      <details>
        <summary>Create project</summary>
        <form onSubmit={(event) => void create(event)}>
          <label>
            Project name
            <input name="name" required maxLength={200} />
          </label>
          <label>
            Data region
            <input name="region" required maxLength={64} placeholder="us-east" />
          </label>
          <button type="submit">Create and provision</button>
        </form>
      </details>
    </section>
  );
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

  return (
    <section aria-labelledby="project-title">
      <button
        type="button"
        className="back-link"
        disabled={project === null}
        onClick={() => project !== null && onBack(project.teamId)}
      >
        ← Team
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Project</p>
          <h1 id="project-title">{project?.name ?? "Loading…"}</h1>
        </div>
        {project === null ? null : <LifecycleBadge state={project.state} />}
      </div>
      <ApiFailureNotice failure={failure} />
      {project === null ? (
        <p>Loading project…</p>
      ) : (
        <>
          <LifecyclePanel
            project={project}
            onSuspend={() => void projectAction("suspend")}
            onRestore={() => void projectAction("restore")}
            onDelete={() => void projectAction("delete")}
          />
          <HealthPanel page={health} />
          <section className="panel full-span" aria-labelledby="environments-title">
            <h2 id="environments-title">Environments</h2>
            <div className="resource-list">
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
        </>
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
    <section className="panel lifecycle-panel" aria-labelledby="lifecycle-title">
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
        <button type="button" onClick={onSuspend} disabled={project.state !== "active"}>
          Suspend
        </button>
        <button
          type="button"
          className="secondary"
          onClick={onRestore}
          disabled={!(["suspended", "deletion_grace"] as string[]).includes(project.state)}
        >
          Restore
        </button>
        <button
          type="button"
          className="danger"
          onClick={onDelete}
          disabled={!(["active", "suspended", "failed"] as string[]).includes(project.state)}
        >
          Request deletion
        </button>
      </div>
    </section>
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
  return (
    <article className="environment-row">
      <div>
        <strong>{environment.name}</strong>
        <code>{environment.id}</code>
        {environment.deletionDeadline === undefined ? null : (
          <small>Restorable until {new Date(environment.deletionDeadline).toLocaleString()}</small>
        )}
      </div>
      <LifecycleBadge state={environment.state} />
      <div className="button-row">
        <button type="button" disabled={environment.state !== "active"} onClick={onOpenWorkspace}>
          Workspace
        </button>
        <button type="button" disabled={environment.state !== "active"} onClick={onOpenCollections}>
          Collections
        </button>
        <button
          type="button"
          className="secondary"
          disabled={environment.state !== "active"}
          onClick={onOpenFunctions}
        >
          Functions
        </button>
        <button
          type="button"
          className="secondary"
          disabled={environment.state !== "active"}
          onClick={onOpenObservability}
        >
          Observability
        </button>
        <button
          type="button"
          className="secondary"
          disabled={environment.state !== "active"}
          onClick={onOpenUsers}
        >
          Users
        </button>
        <button
          type="button"
          className="secondary"
          disabled={environment.state !== "active"}
          onClick={onOpenSecurity}
        >
          Credentials
        </button>
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
    </article>
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
    <section className="panel" aria-labelledby="health-title">
      <h2 id="health-title">Data-plane health</h2>
      {page === null ? (
        <p>No environment health is available yet.</p>
      ) : health.length === 0 ? (
        <p>No retained health observations.</p>
      ) : (
        <ul className="signal-list">
          {health.map((record) => {
            const payload = record.payload;
            return (
              <li key={`${record.timestamp}-${payload.service}-${payload.region}`}>
                <strong>{payload.service}</strong> in {payload.region}: {payload.status}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

export function LifecycleBadge({ state }: { readonly state: string }) {
  return (
    <span className={`status status-${state}`}>
      <span className="visually-hidden">Status: </span>
      {state.replaceAll("_", " ")}
    </span>
  );
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}

function deletionConfirmation(name: string | undefined): string {
  return `delete:${name ?? "resource"}`;
}
