import type { Environment } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import type { TableColumn } from "../cli/output.js";
import type { Command } from "../cli/registry.js";
import { PROJECT_OPTION, projectFrom, settleLifecycle } from "./shared.js";

const ENVIRONMENT_ID = { name: "env-id", description: "Environment id", required: true } as const;

const ENVIRONMENT_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "name" },
  { key: "state" },
  { key: "createdAt" },
  { key: "updatedAt" },
];

/** Applies --wait, prints the environment, and points at the undo for a deletion. */
async function settleAndPrint(context: CommandContext, environment: Environment): Promise<void> {
  const client = await context.management();
  const settled = await settleLifecycle(
    context,
    environment,
    () => client.getEnvironment(environment.projectId, environment.id),
    "environment",
  );
  context.out(settled);
  if (!context.json && settled.deletionDeadline !== undefined) {
    context.info(
      `Environment ${settled.id} is in its deletion grace period until ${settled.deletionDeadline}; ` +
        `undo with: mako envs restore ${settled.id} --project ${settled.projectId}`,
    );
  }
}

export const envsCommands: readonly Command[] = [
  {
    path: ["envs", "list"],
    summary: "List a project's environments",
    operations: ["listEnvironments"],
    options: PROJECT_OPTION,
    run: async (context, args) => {
      const projectId = projectFrom(context, args);
      const client = await context.management();
      context.out(await client.listEnvironments(projectId), { columns: ENVIRONMENT_COLUMNS });
    },
  },
  {
    path: ["envs", "create"],
    summary: "Create an environment in a project (--wait for provisioning)",
    operations: ["createEnvironment", "getEnvironment"],
    positionals: [{ name: "name", description: "Environment name", required: true }],
    options: PROJECT_OPTION,
    run: async (context, args) => {
      const projectId = projectFrom(context, args);
      const name = args.requirePositional(0, "name");
      const client = await context.management();
      await settleAndPrint(
        context,
        await client.createEnvironment(projectId, name, context.idempotencyKey()),
      );
    },
  },
  {
    path: ["envs", "get"],
    summary: "Show an environment",
    operations: ["getEnvironment"],
    positionals: [ENVIRONMENT_ID],
    options: PROJECT_OPTION,
    run: async (context, args) => {
      const projectId = projectFrom(context, args);
      const client = await context.management();
      context.out(await client.getEnvironment(projectId, args.requirePositional(0, "env-id")));
    },
  },
  {
    path: ["envs", "suspend"],
    summary: "Suspend an environment; it stops serving",
    operations: ["suspendEnvironment", "getEnvironment"],
    positionals: [ENVIRONMENT_ID],
    options: PROJECT_OPTION,
    destructive: {
      action: "suspend environment",
      resource: (args) => args.requirePositional(0, "env-id"),
    },
    run: async (context, args) => {
      const projectId = projectFrom(context, args);
      const environmentId = args.requirePositional(0, "env-id");
      const client = await context.management();
      await settleAndPrint(
        context,
        await client.suspendEnvironment(projectId, environmentId, context.idempotencyKey()),
      );
    },
  },
  {
    path: ["envs", "restore"],
    summary: "Restore a suspended environment or one in its deletion grace period",
    operations: ["restoreEnvironment", "getEnvironment"],
    positionals: [ENVIRONMENT_ID],
    options: PROJECT_OPTION,
    run: async (context, args) => {
      const projectId = projectFrom(context, args);
      const environmentId = args.requirePositional(0, "env-id");
      const client = await context.management();
      await settleAndPrint(
        context,
        await client.restoreEnvironment(projectId, environmentId, context.idempotencyKey()),
      );
    },
  },
  {
    path: ["envs", "delete"],
    summary: "Start an environment's deletion grace period",
    operations: ["requestEnvironmentDeletion", "getEnvironment"],
    positionals: [ENVIRONMENT_ID],
    options: PROJECT_OPTION,
    destructive: {
      action: "delete environment",
      resource: (args) => args.requirePositional(0, "env-id"),
    },
    run: async (context, args) => {
      const projectId = projectFrom(context, args);
      const environmentId = args.requirePositional(0, "env-id");
      const client = await context.management();
      await settleAndPrint(
        context,
        await client.requestEnvironmentDeletion(projectId, environmentId, environmentId),
      );
    },
  },
];
