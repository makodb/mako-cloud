import { useCallback, useEffect, useState } from "react";

import type {
  EmailTemplate,
  EmailTemplateKind,
  EmailTemplateRender,
} from "@mako-cloud/management-sdk";

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
    <section aria-labelledby="email-templates-title" className="email-templates-screen">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="email-templates-title">Email templates</h1>
          <p>
            The emails this environment sends its users. Plain text with{" "}
            <code>{"{{variable}}"}</code> placeholders; no HTML, scripts, or remote content. Each
            kind has a built-in default until you save your own.
          </p>
        </div>
      </div>
      {failure === null ? null : <ApiFailureNotice failure={failure} />}
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
      {templates === null && failure === null ? <p role="status">Loading templates…</p> : null}

      <div className="template-layout">
        <nav aria-label="Template kinds" className="template-kinds">
          <ul>
            {TEMPLATE_KINDS.map((entry) => {
              const template = templates?.find((candidate) => candidate.kind === entry.kind);
              return (
                <li key={entry.kind}>
                  <button
                    type="button"
                    className={entry.kind === selected ? "active" : ""}
                    aria-current={entry.kind === selected ? "true" : undefined}
                    onClick={() => setSelected(entry.kind)}
                  >
                    {entry.label}
                    <span className="muted">
                      {template === undefined
                        ? ""
                        : template.isDefault
                          ? " · default"
                          : ` · customized v${template.version}`}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </nav>

        <div className="template-editor">
          <h2 id="template-editor-heading">{labelFor(selected)}</h2>
          <p className="muted">
            Sent {TEMPLATE_KINDS.find((entry) => entry.kind === selected)?.sentWhen}. Variables:{" "}
            {variablesFor(selected).map((variable, index) => (
              <span key={variable}>
                {index === 0 ? "" : ", "}
                <code>{`{{${variable}}}`}</code>
              </span>
            ))}
            .
          </p>
          {current === null ? null : (
            <p className="muted" data-testid="template-state">
              {current.isDefault
                ? "Using the built-in default."
                : `Customized (version ${current.version}${current.updatedAt === null ? "" : `, saved ${current.updatedAt}`}).`}
            </p>
          )}
          <label>
            Subject
            <input
              value={subject}
              maxLength={200}
              onChange={(event) => {
                setSubject(event.currentTarget.value);
                setDirty(true);
                setStatus(null);
              }}
            />
          </label>
          <label>
            Body
            <textarea
              rows={12}
              value={textBody}
              maxLength={32768}
              onChange={(event) => {
                setTextBody(event.currentTarget.value);
                setDirty(true);
                setStatus(null);
              }}
            />
          </label>
          <div className="button-row">
            <button type="button" onClick={() => void save()} disabled={busy !== null || !dirty}>
              {busy === "save" ? "Saving…" : "Save template"}
            </button>
            <button
              type="button"
              className="secondary"
              onClick={() => void render()}
              disabled={busy !== null}
            >
              {busy === "preview" ? "Rendering…" : "Preview"}
            </button>
            <button
              type="button"
              className="secondary danger"
              onClick={() => void reset()}
              disabled={busy !== null || current === null || current.isDefault}
            >
              {busy === "reset" ? "Resetting…" : "Reset to default"}
            </button>
          </div>
          {preview === null ? null : (
            <section aria-labelledby="template-preview-heading" className="template-preview">
              <h3 id="template-preview-heading">Preview with placeholder data</h3>
              <p>
                <strong>Subject:</strong>{" "}
                <span data-testid="preview-subject">{preview.subject}</span>
              </p>
              <pre data-testid="preview-body">{preview.textBody}</pre>
            </section>
          )}
        </div>
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
