# Developer console

The console is what a developer sees from sign-in onward. It is organised in
three levels, each with the same navigation shape, so that any product area of
any environment is at most two selections from the home page.

## Home

Signing in lands on the home dashboard: every project the developer can reach,
across their personal space ("Your projects") and every team they belong to, as
cards carrying lifecycle state, region, plan, and headline usage for the current
period, with recent activity beside them. Each card loads its summaries on its
own, so one project whose usage cannot be read marks only its own card. A
developer with no projects is offered a guided first run — create a project,
wait for it to become active, copy its keys, check a connection — that can be
dismissed and reopened, and resumes where it was left. Its progress is kept in the
browser tab's session storage, never on the server and never in durable local
storage, so a closed tab simply shows the guide again when there is still
nothing to show.

## Project

A project's home summarises its environments and their readiness, the selected
environment's API URL, public key, and quickstart, its usage against quota,
data-plane health, and recent activity, and offers Overview, Usage, Activity,
and Settings alongside the environment list. Settings show the owner, region, identifiers, and lifecycle, and offer the three
changes an owner may make: renaming the project, transferring it between the
personal space and the teams the developer administers, and requesting deletion
with its grace period. Each asks for confirmation and is audited; a transfer
keeps the identifier, environments, data, policies, users, keys, and functions,
holds every environment to the new owner's plan before the owner changes, and
is recorded under both the previous and the new owner.

Routes: `/projects/{projectId}` and `/projects/{projectId}/{overview|usage|activity|settings}`.

## Environment

Inside an environment the sidebar lists every destination the platform
authorises for the developer — Overview, Data, Collections, Sync, Users,
Policies, Functions, Observability, Backups, API & Connect, Settings — plus
the destinations the console serves from signals the API already exposes:
Storage (the environment's buckets with their object counts and stored bytes,
each bucket's access, size and content-type limits, and rules, and its
objects — listed by prefix, paged, and deletable), Webhooks (the
environment's webhook endpoints with their subscriptions, state, and failure
count; registration with a URL, description, per-collection event
subscriptions, and enabled flag, after which the platform-generated signing
secret is shown once in a dismissable panel with copy guidance and never
again; each endpoint's details, settings, enable/disable, a confirmed secret
rotation that shows the new secret once, resume when the platform paused it
after sustained failure — with the reason shown — a confirmed delete, and its
delivery log newest first, filtered by state, paged, with time, event,
collection, document, attempts, status, and error per delivery and
redelivery of a failed one), Auth providers (the
environment's sign-in settings edited as one unit: OpenID Connect and GitHub
providers with their client identifiers, scopes, and enabled state, the
redirect allowlist, and magic links; a client secret is typed once, sent once,
and never displayed again — the screen only says whether one is stored, and a
provider submitted without a stored or typed secret is refused before anything
is sent), Email templates (the four application emails — verification,
recovery, invitation, magic link — with the variables each may use, a
server-rendered preview with placeholder data, save per kind, and a confirmed
reset to the built-in default), Logs (retained, scrubbed function output),
Usage (this month's meters against the plan), and Activity (the audit trail as
the developer may read it).

A function's own page carries its Schedules: every cron schedule attached to
the function with its expression (five fields, evaluated in UTC), state, next
run, and last run's outcome, response status, and duration; a form that
attaches one — name, expression with the syntax and examples beside it,
method, path, content type, body, headers, and whether it starts enabled —
where an invalid expression is refused by the API at save time and shown with
its message; per schedule, pause and resume (a paused schedule keeps its
history and shows no next run), run now (queued outside the cron times and
recorded as manual), and a confirmed delete; and, opened beneath a schedule,
its run history newest first, filtered by outcome and paged, with due time,
start, duration, outcome — including runs skipped because the previous one
was still executing — response status, error, and whether the run was manual.

Product areas the deployment does not provide yet — Domains — are listed and
disabled with a reason, never hidden: the shape of the product is visible
before every area is enabled.

Routes: `/projects/{projectId}/environments/{environmentId}/{storage|webhooks|auth-providers|email-templates|logs|usage|activity}`,
`…/storage/{bucketId}`, and `…/webhooks/{webhookId}` join the existing environment routes; every pre-existing deep link keeps working and
opens inside the shell with its context shown.

## Context and accessibility

The active team, project, environment, and developer identity stay visible in
the shell. URLs carry only safe identifiers and destination names. The
navigation is keyboard-operable and the current destination is announced to
assistive technology through `aria-current`.
