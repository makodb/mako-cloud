import type { EmailTemplateKind } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

const KINDS: readonly EmailTemplateKind[] = [
  "verification",
  "recovery",
  "invitation",
  "magic_link",
];

const TEMPLATE_COLUMNS: readonly TableColumn[] = [
  { key: "kind" },
  { key: "isDefault", label: "default" },
  { key: "version" },
  { key: "subject" },
  { key: "updatedAt" },
];

const KIND: PositionalSpec = {
  name: "kind",
  description: `Template kind: ${KINDS.join(", ")}`,
  required: true,
};

/** The text options `set` and `preview` share. */
const TEXT_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  subject: {
    type: "string",
    description: "One-line subject; may use {{variables}}",
    placeholder: "<text>",
  },
  body: {
    type: "string",
    description: "Plain-text body: @path, - for stdin, or inline text; may use {{variables}}",
    placeholder: "<@file|-|text>",
  },
};

function kindOf(args: CommandArgs): EmailTemplateKind {
  const kind = args.requirePositional(0, "kind");
  if (!KINDS.includes(kind as EmailTemplateKind)) {
    throw usageError(`<kind> must be one of ${KINDS.join(", ")}`);
  }
  return kind as EmailTemplateKind;
}

async function bodyFrom(context: CommandContext, args: CommandArgs): Promise<string | undefined> {
  const source = args.string("body");
  if (source === undefined) return undefined;
  return context.readInput(source);
}

async function listTemplates(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const templates = await client.listEmailTemplates(projectId, environmentId);
  context.out(templates, { columns: TEMPLATE_COLUMNS });
}

async function getTemplate(context: CommandContext, args: CommandArgs): Promise<void> {
  const kind = kindOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getEmailTemplate(projectId, environmentId, kind));
}

async function setTemplate(context: CommandContext, args: CommandArgs): Promise<void> {
  const kind = kindOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const subject = args.string("subject");
  const textBody = await bodyFrom(context, args);
  if (subject === undefined || textBody === undefined) {
    throw usageError("--subject <text> and --body <@file|-|text> are both required");
  }
  const client = await context.management();
  const template = await client.updateEmailTemplate(
    projectId,
    environmentId,
    kind,
    { subject, textBody },
    context.idempotencyKey(),
  );
  context.out(template);
}

async function resetTemplate(context: CommandContext, args: CommandArgs): Promise<void> {
  const kind = kindOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const template = await client.resetEmailTemplate(projectId, environmentId, kind);
  context.info(`${kind} template reset to the built-in default`);
  context.out(template);
}

async function previewTemplate(context: CommandContext, args: CommandArgs): Promise<void> {
  const kind = kindOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const subject = args.string("subject");
  const textBody = await bodyFrom(context, args);
  const client = await context.management();
  const rendered = await client.previewEmailTemplate(projectId, environmentId, kind, {
    ...(subject !== undefined ? { subject } : {}),
    ...(textBody !== undefined ? { textBody } : {}),
  });
  if (context.json) {
    context.out(rendered);
    return;
  }
  // A rendered mail reads best as a mail, not as a key/value table.
  context.out(`Subject: ${rendered.subject}\n\n${rendered.textBody}`);
}

export const emailTemplatesCommands: readonly Command[] = [
  {
    path: ["email-templates", "list"],
    summary:
      "List the environment's application email templates; defaults are marked until customized",
    operations: ["listEmailTemplates"],
    options: { ...TENANT_OPTIONS },
    run: listTemplates,
  },
  {
    path: ["email-templates", "get"],
    summary: "Show one email template's subject and body as they are in effect",
    operations: ["getEmailTemplate"],
    positionals: [KIND],
    options: { ...TENANT_OPTIONS },
    run: getTemplate,
  },
  {
    path: ["email-templates", "set"],
    summary:
      "Customize an email template's subject and plain-text body; unknown {{variables}} are refused",
    operations: ["updateEmailTemplate"],
    positionals: [KIND],
    options: { ...TENANT_OPTIONS, ...TEXT_OPTIONS },
    run: setTemplate,
  },
  {
    path: ["email-templates", "reset"],
    summary: "Reset an email template to the built-in default",
    operations: ["resetEmailTemplate"],
    positionals: [KIND],
    options: { ...TENANT_OPTIONS },
    run: resetTemplate,
  },
  {
    path: ["email-templates", "preview"],
    summary:
      "Render an email template with placeholder data; --subject or --body previews unsaved text",
    operations: ["previewEmailTemplate"],
    positionals: [KIND],
    options: { ...TENANT_OPTIONS, ...TEXT_OPTIONS },
    run: previewTemplate,
  },
];
