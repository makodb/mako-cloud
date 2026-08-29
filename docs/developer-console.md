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
objects — listed by prefix, paged, and deletable), Logs (retained, scrubbed
function output), Usage (this month's meters against the plan), and Activity
(the audit trail as the developer may read it).

Product areas the deployment does not provide yet — Auth providers, Webhooks,
Schedules, Domains — are listed and disabled with a reason, never hidden: the
shape of the product is visible before every area is enabled.

Routes: `/projects/{projectId}/environments/{environmentId}/{storage|logs|usage|activity}`
and `…/storage/{bucketId}` join the existing environment routes; every pre-existing deep link keeps working and
opens inside the shell with its context shown.

## Context and accessibility

The active team, project, environment, and developer identity stay visible in
the shell. URLs carry only safe identifiers and destination names. The
navigation is keyboard-operable and the current destination is announced to
assistive technology through `aria-current`.
