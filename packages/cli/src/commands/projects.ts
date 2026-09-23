import type { Project, Team } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { CLI_NAME } from "../cli/name.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { settleLifecycle } from "./shared.js";

const PROJECT_ID = { name: "project-id", description: "Project id", required: true } as const;

const TEAM_OPTION: Readonly<Record<string, OptionSpec>> = {
  team: {
    type: "string",
    description: "Team id (default: your personal space)",
    placeholder: "<team-id>",
  },
};

const TRANSFER_TEAM_OPTION: Readonly<Record<string, OptionSpec>> = {
  team: {
    type: "string",
    description: "Team that receives the project (default: your personal space)",
    placeholder: "<team-id>",
  },
};

const PROJECT_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "name" },
  { key: "teamId" },
  { key: "region" },
  { key: "state" },
  { key: "createdAt" },
];

const ALL_PROJECT_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "name" },
  { key: "teamName" },
  { key: "teamId" },
  { key: "region" },
  { key: "state" },
];

/** Personal space first, then teams in the API's order. */
function personalFirst(teams: readonly Team[]): Team[] {
  return [...teams].sort((a, b) => Number(b.kind === "personal") - Number(a.kind === "personal"));
}

async function list(context: CommandContext, args: CommandArgs): Promise<void> {
  const client = await context.management();
  const teamId = args.string("team");
  if (teamId !== undefined) {
    context.out(await client.listProjects(teamId), { columns: PROJECT_COLUMNS });
    return;
  }
  const rows: (Project & { readonly teamName: string })[] = [];
  for (const team of personalFirst(await client.listTeams())) {
    for (const project of await client.listProjects(team.id)) {
      rows.push({ ...project, teamName: team.name });
    }
  }
  context.out(rows, { columns: ALL_PROJECT_COLUMNS });
}

/** Applies --wait, prints the project, and points at the undo for a deletion. */
async function settleAndPrint(context: CommandContext, project: Project): Promise<void> {
  const client = await context.management();
  const settled = await settleLifecycle(
    context,
    project,
    () => client.getProject(project.id),
    "project",
  );
  context.out(settled);
  if (!context.json && settled.deletionDeadline !== undefined) {
    context.info(
      `Project ${settled.id} is in its deletion grace period until ${settled.deletionDeadline}; ` +
        `undo with: ${CLI_NAME} projects restore ${settled.id}`,
    );
  }
}

export const projectsCommands: readonly Command[] = [
  {
    path: ["projects", "list"],
    summary: "List projects in one team, or in every team you belong to",
    operations: ["listProjects", "listTeams"],
    options: TEAM_OPTION,
    run: list,
  },
  {
    path: ["projects", "create"],
    summary: "Create a project in your personal space or a team (--wait for provisioning)",
    operations: ["createProject", "getProject"],
    positionals: [{ name: "name", description: "Project name", required: true }],
    options: {
      region: { type: "string", description: "Region id", placeholder: "<region>", required: true },
      ...TEAM_OPTION,
    },
    run: async (context, args) => {
      const teamId = args.string("team");
      const input = {
        ...(teamId !== undefined ? { teamId } : {}),
        name: args.requirePositional(0, "name"),
        region: args.requireString("region"),
      };
      const client = await context.management();
      await settleAndPrint(context, await client.createProject(input, context.idempotencyKey()));
    },
  },
  {
    path: ["projects", "get"],
    summary: "Show a project",
    operations: ["getProject"],
    positionals: [PROJECT_ID],
    run: async (context, args) => {
      const client = await context.management();
      context.out(await client.getProject(args.requirePositional(0, "project-id")));
    },
  },
  {
    path: ["projects", "suspend"],
    summary: "Suspend a project; its environments stop serving",
    operations: ["suspendProject", "getProject"],
    positionals: [PROJECT_ID],
    destructive: {
      action: "suspend project",
      resource: (args) => args.requirePositional(0, "project-id"),
    },
    run: async (context, args) => {
      const projectId = args.requirePositional(0, "project-id");
      const client = await context.management();
      await settleAndPrint(
        context,
        await client.suspendProject(projectId, context.idempotencyKey()),
      );
    },
  },
  {
    path: ["projects", "restore"],
    summary: "Restore a suspended project or one in its deletion grace period",
    operations: ["restoreProject", "getProject"],
    positionals: [PROJECT_ID],
    run: async (context, args) => {
      const projectId = args.requirePositional(0, "project-id");
      const client = await context.management();
      await settleAndPrint(
        context,
        await client.restoreProject(projectId, context.idempotencyKey()),
      );
    },
  },
  {
    path: ["projects", "delete"],
    summary: "Start a project's deletion grace period",
    operations: ["requestProjectDeletion", "getProject"],
    positionals: [PROJECT_ID],
    destructive: {
      action: "delete project",
      resource: (args) => args.requirePositional(0, "project-id"),
    },
    run: async (context, args) => {
      const projectId = args.requirePositional(0, "project-id");
      const client = await context.management();
      await settleAndPrint(context, await client.requestProjectDeletion(projectId, projectId));
    },
  },
  {
    path: ["projects", "rename"],
    summary: "Rename a project",
    operations: ["updateProject"],
    positionals: [PROJECT_ID, { name: "name", description: "New project name", required: true }],
    run: async (context, args) => {
      const client = await context.management();
      context.out(
        await client.updateProject(
          args.requirePositional(0, "project-id"),
          args.requirePositional(1, "name"),
        ),
      );
    },
  },
  {
    path: ["projects", "transfer"],
    summary: "Move a project to a team you administer, or to your personal space",
    operations: ["transferProject"],
    positionals: [PROJECT_ID],
    options: TRANSFER_TEAM_OPTION,
    destructive: {
      action: "transfer project",
      resource: (args) => args.requirePositional(0, "project-id"),
    },
    run: async (context, args) => {
      const projectId = args.requirePositional(0, "project-id");
      const client = await context.management();
      const moved = await client.transferProject(projectId, args.string("team"), projectId);
      context.out(moved);
      if (!context.json) {
        context.info(
          `Project ${moved.id} now belongs to ${moved.teamId}; ` +
            "the transfer is audited under both the previous and the new owner.",
        );
      }
    },
  },
];
