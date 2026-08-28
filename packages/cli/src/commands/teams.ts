import type { Team, TeamMembership, TeamRole } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { expiresAtFrom } from "./auth-tokens.js";
import { SECRET_FILE_OPTION } from "./shared.js";

/** Typed as a complete record so the build fails when the API grows a role. */
const ROLE_SET: Readonly<Record<TeamRole, true>> = {
  owner: true,
  administrator: true,
  developer: true,
  viewer: true,
};

export const TEAM_ROLES = Object.keys(ROLE_SET) as readonly TeamRole[];

function roleFrom(args: CommandArgs): TeamRole {
  const value = args.requireString("role");
  if (!Object.hasOwn(ROLE_SET, value)) {
    throw usageError(`unknown role "${value}"; valid roles: ${TEAM_ROLES.join(", ")}`);
  }
  return value as TeamRole;
}

const ROLE_OPTION: Readonly<Record<string, OptionSpec>> = {
  role: {
    type: "string",
    description: `Team role: ${TEAM_ROLES.join(", ")}`,
    placeholder: "<role>",
    required: true,
  },
};

const TEAM_ID = { name: "team-id", description: "Team id", required: true } as const;
const DEVELOPER_ID = {
  name: "developer-id",
  description: "Developer identity id of the member",
  required: true,
} as const;

const TEAM_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "name" },
  { key: "kind" },
  { key: "state" },
  { key: "createdAt" },
];

const MEMBER_COLUMNS: readonly TableColumn[] = [
  { key: "developerIdentityId" },
  { key: "role" },
  { key: "createdAt" },
  { key: "updatedAt" },
];

const LINE_ITEM_COLUMNS: readonly TableColumn[] = [
  { key: "resource" },
  { key: "quantity" },
  { key: "included" },
  { key: "overage" },
  { key: "amount" },
];

const PERIOD = /^\d{4}-(?:0[1-9]|1[0-2])$/u;

function dollars(microDollars: number): string {
  return `$${(microDollars / 1_000_000).toFixed(2)}`;
}

function noteDeletion(context: CommandContext, team: Team): void {
  if (context.json || team.deletionDeadline === undefined) return;
  context.info(
    `Team ${team.id} is in its deletion grace period until ${team.deletionDeadline}; ` +
      `undo with: mako teams restore ${team.id}`,
  );
}

async function bill(context: CommandContext, args: CommandArgs): Promise<void> {
  const teamId = args.requirePositional(0, "team-id");
  const period = args.string("period");
  if (period !== undefined && !PERIOD.test(period)) {
    throw usageError("--period must be a calendar month as YYYY-MM");
  }
  const client = await context.management();
  const statement = await client.getTeamBill(teamId, period);
  if (context.json) {
    context.out(statement);
    return;
  }
  // The notice qualifies every figure below it, so it is read first.
  context.io.stdout.write(`${statement.notice}\n\n`);
  context.out({
    teamId: statement.teamId,
    planId: statement.planId,
    period: `${statement.periodStart} to ${statement.periodEnd}`,
    finalized: statement.finalized,
    closedAt: statement.closedAt ?? null,
    observedAt: statement.observedAt,
    base: dollars(statement.baseMicroDollars),
    total: dollars(statement.totalMicroDollars),
    credits: dollars(statement.creditsMicroDollars),
    balance: dollars(statement.balanceMicroDollars),
    collectable: statement.collectable,
  });
  if (statement.lineItems.length > 0) {
    context.io.stdout.write("\n");
    context.out(
      statement.lineItems.map((item) => ({
        resource: item.resource,
        quantity: item.quantity,
        included: item.included,
        overage: item.overage,
        amount: dollars(item.amountMicroDollars),
      })),
      { columns: LINE_ITEM_COLUMNS },
    );
  }
}

export const teamsCommands: readonly Command[] = [
  {
    path: ["teams", "list"],
    summary: "List the teams you belong to, including your personal space",
    operations: ["listTeams"],
    run: async (context) => {
      const client = await context.management();
      context.out(await client.listTeams(), { columns: TEAM_COLUMNS });
    },
  },
  {
    path: ["teams", "create"],
    summary: "Create a team",
    operations: ["createTeam"],
    positionals: [{ name: "name", description: "Team name", required: true }],
    run: async (context, args) => {
      const client = await context.management();
      context.out(await client.createTeam(args.requirePositional(0, "name")));
    },
  },
  {
    path: ["teams", "get"],
    summary: "Show a team",
    operations: ["getTeam"],
    positionals: [TEAM_ID],
    run: async (context, args) => {
      const client = await context.management();
      context.out(await client.getTeam(args.requirePositional(0, "team-id")));
    },
  },
  {
    path: ["teams", "rename"],
    summary: "Rename a team",
    operations: ["updateTeam"],
    positionals: [TEAM_ID, { name: "name", description: "New team name", required: true }],
    run: async (context, args) => {
      const client = await context.management();
      context.out(
        await client.updateTeam(
          args.requirePositional(0, "team-id"),
          args.requirePositional(1, "name"),
        ),
      );
    },
  },
  {
    path: ["teams", "delete"],
    summary: "Start a team's deletion grace period; its projects lose access immediately",
    operations: ["requestTeamDeletion"],
    positionals: [TEAM_ID],
    destructive: {
      action: "delete team",
      resource: (args) => args.requirePositional(0, "team-id"),
    },
    run: async (context, args) => {
      const teamId = args.requirePositional(0, "team-id");
      const client = await context.management();
      const team = await client.requestTeamDeletion(teamId, teamId);
      context.out(team);
      noteDeletion(context, team);
    },
  },
  {
    path: ["teams", "restore"],
    summary: "Restore a team from its deletion grace period",
    operations: ["restoreTeam"],
    positionals: [TEAM_ID],
    run: async (context, args) => {
      const client = await context.management();
      context.out(
        await client.restoreTeam(args.requirePositional(0, "team-id"), context.idempotencyKey()),
      );
    },
  },
  {
    path: ["teams", "bill"],
    summary: "Show a team's bill for the current month or a closed period",
    operations: ["getTeamBill"],
    positionals: [TEAM_ID],
    options: {
      period: {
        type: "string",
        description: "A closed calendar month, as YYYY-MM (default: the live month)",
        placeholder: "<YYYY-MM>",
      },
    },
    run: bill,
  },
  {
    path: ["teams", "members", "list"],
    summary: "List a team's members and their roles",
    operations: ["listTeamMembers"],
    positionals: [TEAM_ID],
    run: async (context, args) => {
      const client = await context.management();
      context.out(await client.listMembers(args.requirePositional(0, "team-id")), {
        columns: MEMBER_COLUMNS,
      });
    },
  },
  {
    path: ["teams", "members", "update"],
    summary: "Change a member's role",
    operations: ["updateTeamMember"],
    positionals: [TEAM_ID, DEVELOPER_ID],
    options: ROLE_OPTION,
    run: async (context, args) => {
      const teamId = args.requirePositional(0, "team-id");
      const developerId = args.requirePositional(1, "developer-id");
      const role = roleFrom(args);
      const client = await context.management();
      context.out(await client.updateMember(teamId, developerId, role));
    },
  },
  {
    path: ["teams", "members", "remove"],
    summary: "Remove a member from a team",
    operations: ["removeTeamMember"],
    positionals: [TEAM_ID, DEVELOPER_ID],
    destructive: {
      action: "remove member",
      resource: (args) => args.requirePositional(1, "developer-id"),
    },
    run: async (context, args) => {
      const teamId = args.requirePositional(0, "team-id");
      const developerId = args.requirePositional(1, "developer-id");
      const client = await context.management();
      await client.removeMember(teamId, developerId);
      if (context.json) context.out({ teamId, developerIdentityId: developerId, removed: true });
      else context.info(`Removed ${developerId} from team ${teamId}.`);
    },
  },
  {
    path: ["teams", "invitations", "create"],
    summary: "Invite a developer to a team; the invitation token is shown once",
    operations: ["createTeamInvitation"],
    positionals: [TEAM_ID],
    options: {
      email: {
        type: "string",
        description: "Email address of the invitee",
        placeholder: "<email>",
        required: true,
      },
      ...ROLE_OPTION,
      "expires-in": {
        type: "string",
        description: "How long the invitation stays open, like 7d or 12h (default: 7d)",
        placeholder: "<duration>",
      },
      ...SECRET_FILE_OPTION,
    },
    run: async (context, args) => {
      const teamId = args.requirePositional(0, "team-id");
      const input = {
        email: args.requireString("email"),
        role: roleFrom(args),
        expiresAt: expiresAtFrom(args, "expires-in", "7d"),
      };
      const client = await context.management();
      const issue = await client.createInvitation(teamId, input);
      await context.secret(
        `invitation token for ${issue.invitation.email}`,
        issue.token,
        context.json ? { invitation: issue.invitation } : { ...issue.invitation },
        args.string("secret-file"),
      );
    },
  },
  {
    path: ["teams", "invitations", "accept"],
    summary: "Join a team with an invitation id and its token",
    operations: ["acceptTeamInvitation"],
    positionals: [
      { name: "invitation-id", description: "Invitation id", required: true },
      {
        name: "token",
        description: "Invitation token; `-` reads it from stdin, `@path` from a file",
        required: true,
      },
    ],
    run: async (context, args) => {
      const invitationId = args.requirePositional(0, "invitation-id");
      const token = (await context.readInput(args.requirePositional(1, "token"))).trim();
      if (token === "") throw usageError("the invitation token is empty");
      const client = await context.management();
      const membership: TeamMembership = await client.acceptInvitation(invitationId, token);
      context.out(membership);
    },
  },
];
