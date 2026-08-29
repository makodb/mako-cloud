# Application mail

Mail an environment sends to its *application users*: address verification,
password recovery, invitations, and magic sign-in links. Developer mail (the
wait-list, verification, and recovery messages for developer accounts) is a
separate path described in [developer registration](developer-registration.md);
application mail shares its relay, its encryption key, and its outbox
discipline, and nothing else.

## How a message flows

1. **The data plane writes an intent.** When an application user asks for a
   magic link, verifies an address, recovers a password, or is invited, the
   data plane does not send mail. It writes a *mail intent* in the
   environment's keyspace: an id (`aml_<32 hex>`), the project and environment
   ids, the kind, the recipient, and a small map of variables. The data plane
   holds the only copy until the control plane has taken it.
2. **The control plane drains intents.** The control plane's mail worker
   thread -- the same one that delivers developer mail -- asks the data plane
   for pending intents over the internal route
   `/_internal/v1/data/application-mail/drain` (lease 60 seconds, at most 32 per
   pass). Drained intents stay leased on the data plane; a lease that expires
   without an acknowledgement is handed out again.
3. **Each intent becomes an outbox record.** For every intent the worker
   resolves the project and environment names from the control store, picks
   the environment's template for the kind (or the built-in default), renders
   it, builds a plain-text envelope, seals it with the developer-mail key,
   and stores it in the control store's **application mail outbox**
   (`control/application-mail-outbox/v1`, one SQLite row per intent). The
   outbox record is keyed by the intent id under a *must-be-absent* write, so
   an intent drained twice produces one record: the second attempt sees a
   conflict, counts as a duplicate, and is still acknowledged. Intake is
   at-least-once from the data plane and exactly-once into the outbox.
4. **Acknowledge.** Once its records are durable, the worker calls
   `/_internal/v1/data/application-mail/acknowledge` with the ids it stored
   (or found already stored). The data plane forgets them. If the data plane
   cannot be reached the pass reports a failure, the intents stay leased, and
   the next pass drains them again -- nothing is lost and nothing is sent
   twice.
5. **Deliver.** A delivery pass then walks the application outbox exactly as
   the developer outbox is walked: lease the record, decrypt the envelope,
   hand it to the SMTP transport with the intent id as the message id, and
   mark it delivered. A transient failure schedules a retry with the same
   exponential backoff (30 s doubling, capped by the configured maximum);
   a permanent failure or the attempt limit dead-letters the record with a
   stable error code. Delivered records age out after the configured
   retention; dead letters after four times that.

Some intents can never become mail: the environment no longer exists, the
recipient is not an address, or the template would not render. Those are
recorded as dead letters *at intake* with codes `environment_not_found`,
`invalid_recipient`, `invalid_template`, or `invalid_envelope`, and
acknowledged, so the data plane stops offering them and the record explains
why. An intent whose id, tenant ids, or kind are malformed is not the control
plane's to record; it is counted as unusable and left with the data plane.

The worker's counters are exported on the developer metrics endpoint as
`mako_application_mail_{stored,delivered,retried,dead_lettered,worker_failures}_total`.
Dead letters at intake count towards `dead_lettered`.

## Templates

Every environment has four templates, one per kind: `verification`,
`recovery`, `invitation`, and `magic_link`. Each is **plain text**: a one-line
subject (1--200 bytes, no line breaks) and a text body (1 byte--32 KiB) with
`{{variable}}` placeholders. Plain text is the applied "safe subset" of the
auth-providers design: the renderer never interprets markup, and mail is
delivered as `text/plain` exactly as rendered, so a template cannot carry
script or remote content.

Templates are validated when they are saved or previewed, never at delivery:

- Only the kind's allowed variables may appear. An unknown `{{name}}` is
  refused with a message naming the field and listing what is allowed.
- Every brace must belong to a well-formed placeholder. A lone `{` or `}`,
  an unclosed `{{`, or a nested brace is refused.
- Placeholder names are lowercase letters and underscores; whitespace inside
  the braces is tolerated (`{{ link }}`).

Rendering substitutes only allowlisted variables. A variable the data plane
did not send renders as empty text (an invitation without an inviter). Values
are sanitized for where they land: control characters never reach the
subject line, and only line breaks and tabs survive in the body, so a
recipient-controlled value cannot inject headers. A subject that renders
blank falls back to the default subject; one that renders past 200 bytes is
cut at a character boundary.

### Variables

| Variable | Kinds | Meaning |
| --- | --- | --- |
| `link` | all | The single-use link the user opens |
| `expires_at` | all | When the link stops working, RFC 3339 |
| `email` | all | The recipient address |
| `project_name` | all | The project's display name, from the control store |
| `environment_name` | all | The environment's display name, from the control store |
| `inviter` | `invitation` | Who sent the invitation; may be empty |

`link`, `expires_at`, `email`, and `inviter` come from the data plane's
intent. `project_name` and `environment_name` are added by the control plane
at render time; anything else in an intent's variables is ignored.

### Defaults

A kind that has not been customized uses a built-in default and reports
`isDefault: true`, `version: 0`, `updatedAt: null`.

| Kind | Default subject |
| --- | --- |
| `verification` | `Verify your email for {{project_name}}` |
| `recovery` | `Reset your {{project_name}} password` |
| `invitation` | `You are invited to {{project_name}}` |
| `magic_link` | `Your sign-in link for {{project_name}}` |

Every default body greets the reader, names the project and environment,
carries `{{link}}` on its own line, states `{{expires_at}}`, and tells a
reader who did not ask for the mail to ignore it. The invitation body adds
`Invited by: {{inviter}}`; the magic-link body says the link can be used
once. The exact text is in
`crates/mako-control-plane/src/email_template.rs`.

### Management API

Templates are environment-scoped management resources. Any member of the
owning team may read and preview them; a role that can change projects
(developer, administrator, owner) may save or reset them. Reads and writes
are audited as `email_template_read` and `email_template_update`.

| Operation | Route |
| --- | --- |
| `listEmailTemplates` | `GET /v1/projects/{p}/environments/{e}/email-templates` -- all four kinds |
| `getEmailTemplate` | `GET .../email-templates/{templateKind}` |
| `updateEmailTemplate` | `PUT .../email-templates/{templateKind}` with `{subject, textBody}` and `Idempotency-Key`; the version advances on every save |
| `resetEmailTemplate` | `DELETE .../email-templates/{templateKind}` -- back to the default; no confirmation header, nothing is destroyed |
| `previewEmailTemplate` | `POST .../email-templates/{templateKind}/actions/preview` with an optional `{subject?, textBody?}` |

A preview renders with placeholder values (`https://app.example.com/...`,
`person@example.com`, `2030-01-01T12:00:00Z`, `A teammate`) and the real
project and environment names. Without a body it renders the template in
effect; with one it renders the unsaved text, and either part may be omitted
to take it from the stored template. Invalid text is refused with a 400 whose
message is the same one a save would give.

From the CLI: `mako email-templates list|get <kind>|set <kind> --subject ...
--body <@file|-|text>|reset <kind>|preview <kind> [--subject] [--body]`; see
[the CLI guide](cli.md).

## Configuration

Application mail needs what developer mail needs: the SMTP relay under
`MAKO_DEVELOPER_SMTP_*` and the mail-encryption secret. Without a relay the
control plane starts, serves the template API, and drains nothing; intents
wait on the data plane until a relay is configured. Application outbox
records are encrypted at rest with the developer-mail key
(`MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF`, or the internal-auth secret
when that is unset) under associated data that binds each record to its
intent id, tenant, and kind. The lease, attempt, backoff, and retention
limits are the developer outbox's (`MAKO_DEVELOPER_OUTBOX_*`,
`MAKO_DEVELOPER_DELIVERED_MAIL_RETENTION_SECONDS`).

### Plaintext SMTP for local deployments

`MAKO_DEVELOPER_SMTP_TLS_MODE=plaintext` speaks unencrypted SMTP to the relay
and is the only mode in which `MAKO_DEVELOPER_SMTP_USERNAME` and the password
reference may be omitted. It exists for the local compose stack's mailpit
(`127.0.0.1:1025`) and for test stubs; a production configuration that names
it is refused at startup with
`CONFIG_INVALID_VALUE at developer_registration.smtp_tls_mode`. Staging and
production use `starttls` or `wrapper` with credentials, as before.

```bash
export MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME=127.0.0.1
export MAKO_DEVELOPER_SMTP_PORT=1025
export MAKO_DEVELOPER_SMTP_TLS_MODE=plaintext
export MAKO_DEVELOPER_SMTP_SENDER="Mako Local <no-reply@localhost>"
```

## Where the code is

- `crates/mako-control-plane/src/email_template.rs` -- kinds, defaults,
  validation, rendering, and the template service with its authorization.
- `crates/mako-control-plane/src/application_mail.rs` -- the outbox record
  and store, the intent source, and the worker that drains, renders, seals,
  acknowledges, delivers, and cleans up.
- `services/mako-control-plane/src/email_template_http.rs` -- the management
  routes; `main.rs` runs the worker on the mail thread; `smtp.rs` builds the
  plaintext transport.
- `crates/mako-internal-rpc` -- the drain and acknowledge contract; the data
  plane serves both routes.
