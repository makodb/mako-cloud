import type {
  EmailTemplate,
  EmailTemplateKind,
  EmailTemplateRender,
} from "@mako-cloud/management-sdk";
import {
  Alert,
  AlertDescription,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  cn,
  Eyebrow,
  Field,
  Input,
  Textarea,
} from "@mako-cloud/ui";
import { CircleCheck } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction } from "./safety.js";

export const TEMPLATE_KINDS: readonly {
  readonly kind: EmailTemplateKind;
  readonly label: string;
  readonly sentWhen: string;
}[] = [
  {
    kind: "verification",
    label: "Address verification",
    sentWhen: "when a user signs up and must confirm the address",
  },
  {
    kind: "recovery",
    label: "Password recovery",
    sentWhen: "when a user asks to reset a forgotten password",
  },
  { kind: "invitation", label: "Invitation", sentWhen: "when a user is invited by another" },
  { kind: "magic_link", label: "Magic link", sentWhen: "when a user asks to sign in by link" },
];

const COMMON_VARIABLES = ["link", "expires_at", "email", "project_name", "environment_name"];

/** A `{{variable}}` placeholder as it appears in a template. */
const VARIABLE = "rounded-sm bg-muted px-1 py-0.5 font-mono text-[0.85em]";

export function variablesFor(kind: EmailTemplateKind): readonly string[] {
  return kind === "invitation" ? [...COMMON_VARIABLES, "inviter"] : COMMON_VARIABLES;
}

// The four emails an environment sends its application users, each with a
// built-in default the developer may replace. Text is edited here, previewed
// by the server with placeholder data, and saved per kind; a reset returns
// the kind to the default.
export function EmailTemplatesScreen({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [templates, setTemplates] = useState<EmailTemplate[] | null>(null);
  const [selected, setSelected] = useState<EmailTemplateKind>("verification");
  const [subject, setSubject] = useState("");
  const [textBody, setTextBody] = useState("");
  const [dirty, setDirty] = useState(false);
  const [preview, setPreview] = useState<EmailTemplateRender | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState<"save" | "preview" | "reset" | null>(null);

  const reload = useCallback(async () => {
    try {
      setTemplates(await client.listEmailTemplates(projectId, environmentId));
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const current = templates?.find((template) => template.kind === selected) ?? null;
  // Selecting a kind (or a reload after save) adopts the stored text.
  useEffect(() => {
    if (current !== null) {
      setSubject(current.subject);
      setTextBody(current.textBody);
      setDirty(false);
      setPreview(null);
    }
  }, [current]);

  const save = async () => {
    setBusy("save");
    setStatus(null);
    try {
      const saved = await client.updateEmailTemplate(
        projectId,
        environmentId,
        selected,
        { subject, textBody },
        idempotencyKey(),
      );
      setTemplates((list) =>
        (list ?? []).map((template) => (template.kind === selected ? saved : template)),
      );
      setFailure(null);
      setStatus(`${labelFor(selected)} template saved (version ${saved.version}).`);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setBusy(null);
    }
  };

  const render = async () => {
    setBusy("preview");
    setStatus(null);
    try {
      setPreview(
        await client.previewEmailTemplate(projectId, environmentId, selected, {
          subject,
          textBody,
        }),
      );
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setBusy(null);
    }
  };

  const reset = async () => {
    if (
      !confirmDestructiveAction({
        action: "Reset template",
        target: labelFor(selected),
        consequence: "Your customized text is discarded and the built-in default is sent again.",
      })
    ) {
      return;
    }
    setBusy("reset");
    setStatus(null);
    try {
      const restored = await client.resetEmailTemplate(projectId, environmentId, selected);
      setTemplates((list) =>
        (list ?? []).map((template) => (template.kind === selected ? restored : template)),
      );
      setFailure(null);
      setStatus(`${labelFor(selected)} template reset to the default.`);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section aria-labelledby="email-templates-title" className="grid gap-6">
      <div className="grid max-w-3xl gap-1">
        <Eyebrow>Environment {environmentId}</Eyebrow>
        <h1 id="email-templates-title" className="text-2xl">
          Email templates
        </h1>
        <p className="m-0 text-sm text-muted-foreground">
          The emails this environment sends its users. Plain text with{" "}
          <code className={VARIABLE}>{"{{variable}}"}</code> placeholders; no HTML, scripts, or
          remote content. Each kind has a built-in default until you save your own.
        </p>
      </div>
      {failure === null ? null : <ApiFailureNotice failure={failure} />}
      {status === null ? null : (
        <Alert variant="positive" role="status">
          <CircleCheck aria-hidden="true" />
          <AlertDescription>{status}</AlertDescription>
        </Alert>
      )}
      {templates === null && failure === null ? (
        <p role="status" className="m-0 text-sm text-muted-foreground">
          Loading templates…
        </p>
      ) : null}

      <div className="grid items-start gap-6 lg:grid-cols-[16rem_minmax(0,1fr)]">
        <nav aria-label="Template kinds">
          <ul className="m-0 grid list-none gap-1 p-0">
            {TEMPLATE_KINDS.map((entry) => {
              const template = templates?.find((candidate) => candidate.kind === entry.kind);
              const active = entry.kind === selected;
              return (
                <li key={entry.kind}>
                  <Button
                    variant="ghost"
                    className={cn(
                      "h-auto w-full justify-start whitespace-normal px-3 py-2 text-left font-normal text-muted-foreground",
                      active && "bg-accent font-medium text-accent-foreground",
                    )}
                    aria-current={active ? "true" : undefined}
                    onClick={() => setSelected(entry.kind)}
                  >
                    {entry.label}
                    <span className="text-xs text-muted-foreground">
                      {template === undefined
                        ? ""
                        : template.isDefault
                          ? " · default"
                          : ` · customized v${template.version}`}
                    </span>
                  </Button>
                </li>
              );
            })}
          </ul>
        </nav>

        <Card>
          <CardHeader>
            <CardTitle id="template-editor-heading">{labelFor(selected)}</CardTitle>
            <p className="m-0 text-sm text-muted-foreground">
              Sent {TEMPLATE_KINDS.find((entry) => entry.kind === selected)?.sentWhen}. Variables:{" "}
              {variablesFor(selected).map((variable, index) => (
                <span key={variable}>
                  {index === 0 ? "" : ", "}
                  <code className={VARIABLE}>{`{{${variable}}}`}</code>
                </span>
              ))}
              .
            </p>
            {current === null ? null : (
              <p className="m-0 text-sm text-muted-foreground" data-testid="template-state">
                {current.isDefault
                  ? "Using the built-in default."
                  : `Customized (version ${current.version}${current.updatedAt === null ? "" : `, saved ${current.updatedAt}`}).`}
              </p>
            )}
          </CardHeader>
          <CardContent className="grid gap-4">
            <Field label="Subject" htmlFor="template-subject">
              <Input
                id="template-subject"
                value={subject}
                maxLength={200}
                onChange={(event) => {
                  setSubject(event.currentTarget.value);
                  setDirty(true);
                  setStatus(null);
                }}
              />
            </Field>
            <Field label="Body" htmlFor="template-body">
              <Textarea
                id="template-body"
                rows={12}
                value={textBody}
                maxLength={32768}
                className="min-h-64 font-mono text-xs leading-relaxed"
                onChange={(event) => {
                  setTextBody(event.currentTarget.value);
                  setDirty(true);
                  setStatus(null);
                }}
              />
            </Field>
            <div className="flex flex-wrap gap-2">
              <Button onClick={() => void save()} disabled={busy !== null || !dirty}>
                {busy === "save" ? "Saving…" : "Save template"}
              </Button>
              <Button variant="outline" onClick={() => void render()} disabled={busy !== null}>
                {busy === "preview" ? "Rendering…" : "Preview"}
              </Button>
              <Button
                variant="outline"
                className="text-destructive hover:text-destructive"
                onClick={() => void reset()}
                disabled={busy !== null || current === null || current.isDefault}
              >
                {busy === "reset" ? "Resetting…" : "Reset to default"}
              </Button>
            </div>
            {preview === null ? null : (
              <section
                aria-labelledby="template-preview-heading"
                className="grid gap-3 rounded-lg border bg-muted/30 p-4"
              >
                <h3 id="template-preview-heading" className="text-sm">
                  Preview with placeholder data
                </h3>
                <p className="m-0 text-sm">
                  <strong>Subject:</strong>{" "}
                  <span data-testid="preview-subject">{preview.subject}</span>
                </p>
                <pre
                  data-testid="preview-body"
                  className="m-0 whitespace-pre-wrap rounded-md border bg-card p-3 font-mono text-xs leading-relaxed"
                >
                  {preview.textBody}
                </pre>
              </section>
            )}
          </CardContent>
        </Card>
      </div>
    </section>
  );
}

function labelFor(kind: EmailTemplateKind): string {
  return TEMPLATE_KINDS.find((entry) => entry.kind === kind)?.label ?? kind;
}

function idempotencyKey(): string {
  return crypto.randomUUID();
}
