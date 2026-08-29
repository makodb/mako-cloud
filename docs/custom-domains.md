# Custom domains

Serve a project's application API and functions on your own hostname, with
a certificate the platform obtains and renews, so applications never expose
the platform's hostname. An authorized member adds a hostname to a project
for one of its environments, proves control of it with a DNS `TXT` record,
and the platform verifies the record, issues a certificate on the first
request, and serves the environment's API and functions on the name. Nothing
is served on a hostname before it is verified, and serving stops when the
proof goes away.

## Adding a domain

`POST /v1/projects/{projectId}/domains` with an idempotency key:

```json
{ "hostname": "api.example.com", "environmentId": "env_..." }
```

Domains are **project-level** resources authorized like project settings:
any member of the project's team reads them, a member who may mutate
projects (developer, administrator, owner) adds, verifies, and removes them.
The environment named in the body must belong to the project in the path.

The response is the domain in state `pending` with the record to publish:

```json
{
  "id": "dom_...",
  "projectId": "prj_...",
  "environmentId": "env_...",
  "hostname": "api.example.com",
  "state": "pending",
  "verification": {
    "recordName": "_mako-verify.api.example.com",
    "recordType": "TXT",
    "recordValue": "mako-domain-verify=<32 random characters>"
  },
  "verifiedAt": null,
  "lastCheckedAt": null,
  "lastError": null,
  "createdAt": "...",
  "updatedAt": "..."
}
```

A hostname is a lowercase DNS name of at least two labels (each 1 to 63
letters, digits, or hyphens, not starting or ending with a hyphen), at most
253 characters, normalized to lowercase without a trailing dot. IP
addresses, `localhost`, and the platform's own public hostname or anything
under it are refused with `400 invalid_request`. A hostname belongs to **at
most one project** on the deployment: adding one that any project already
claims, verified or not, is refused with `409 conflict`. A project may hold
at most 20 domains.

## The verification record

Publish a `TXT` record at `verification.recordName` (`_mako-verify.<hostname>`)
whose value is exactly `verification.recordValue`. Other `TXT` records at
the same name are ignored; a `CNAME` that resolves to the record works too.
The record proves control once; keep it published, because verification is
re-checked for as long as the domain exists.

Separately, point the hostname itself at the platform's address (an `A` or
`AAAA` record, or a `CNAME` to the platform's public hostname). The
verification record alone does not route traffic, and the certificate can
only be issued once the name reaches the platform.

## Verification: the check, its cadence, and the two-check rule

A control-plane worker looks every domain's record up **once a minute**
through the host's DNS resolver (`MAKO_DNS_RESOLVER`, see
[configuration](configuration.md)); `POST
/v1/projects/{projectId}/domains/{domainId}/actions/verify` runs the same
check immediately and returns the domain with the outcome. Each check
records `lastCheckedAt` and, when the record was not found, `lastError`:

| Check found | `pending` domain | `verified` domain | `failed` domain |
| --- | --- | --- | --- |
| The value | becomes `verified`; `verifiedAt` is set once, `lastError` cleared | stays `verified` | becomes `verified` again |
| No `TXT` record (`record_missing`) | stays `pending` | one strike; **two consecutive** strikes make it `failed` | stays `failed` |
| Other values only (`record_mismatch`) | stays `pending` | one strike, as above | stays `failed` |
| No answer (`dns_unavailable`) | no change | no change, no strike | no change |

A `verified` domain becomes `failed` -- and serving stops -- only after two
checks in a row that answered and did not find the record. One miss is a
strike, not a revocation, so a resolver hiccup, a propagating change, or a
single bad answer between two good checks never takes a production hostname
down; a check the resolver could not answer at all (timeout, server failure,
truncated answer) is neither a strike nor a reset. A `failed` domain keeps
its `verifiedAt` for the record and is served again as soon as the record
is back.

The control plane counts checks as
`mako_custom_domain_checks_total{outcome="verified|record_missing|record_mismatch|dns_unavailable"}`,
revocations as `mako_custom_domain_revocations_total`, and publication
failures as `mako_custom_domain_publish_failures_total`.

## What a domain serves, and what it never serves

A verified hostname serves exactly two things for its environment:

- the **application data-plane routes** -- `/v1/projects/{projectId}/environments/{environmentId}/...`
  for auth, documents, replication, and storage, with the same paths, keys,
  and tokens as on the platform hostname;
- **function invocations** as `/functions/v1/{functionName}` -- without
  the project reference, because the hostname names the environment.

Everything else answers `404` on a custom domain: the console, the
management API, the operator API, the developer workspace routes, the
service-credential routes, and the platform's function path shape. A request
on a custom domain whose path names a different project or environment than
the one the hostname is verified for is refused with `404 not_found`, and a
function is served only when the hostname is in the verified list the
control plane resolves for that environment. Serving is gated at every hop:
the reverse proxy sets `X-Mako-Custom-Domain` on the custom-domain listener
only and strips it on the platform hostname; the data plane checks it
against the verified list the control plane installs for each environment;
the edge gateway checks it against the route's verified list.

## Certificates: on-demand TLS and the ask gate

Certificates are obtained by the reverse proxy **on demand**, at the first
TLS handshake for a hostname, and renewed automatically. Before it requests
one, the proxy asks the control plane's loopback-only
`GET /_internal/v1/custom-domains/ask?domain=<hostname>` endpoint, which
answers `200` for a hostname in state `verified` and `404` for anything
else -- so no certificate is ever requested for a hostname nobody verified,
and a `failed` or removed domain is refused at the next handshake. On the
public beta the staging certificate authority is in use, so browsers do not
trust the certificates issued there; that is a property of the beta, not of
custom domains.

## Removing a domain

`DELETE /v1/projects/{projectId}/domains/{domainId}` with an idempotency key
answers `204`. Serving on the hostname stops: the environment's verified
list is republished without it, the ask endpoint stops answering for it, and
its certificate is no longer renewed. The hostname is free to be claimed by
any project again.

## Reading domains

`GET /v1/projects/{projectId}/domains` lists a project's domains, oldest
first; `GET /v1/projects/{projectId}/domains/{domainId}` reads one. Create,
verify, and delete are recorded in the control audit log as
`custom_domain_create`, `custom_domain_verify`, and `custom_domain_delete`.

## Testing verification locally

The control plane resolves through whatever `MAKO_DNS_RESOLVER` names. The
smoke harness ships a loopback DNS stub (`mako_smoke::DnsStub`) that answers
`TXT` questions from a table and `NXDOMAIN` otherwise; start it, set
`MAKO_DNS_RESOLVER=127.0.0.1:<port>` for the control plane, publish the
record with `set_txt`, verify, `clear` it, and watch re-verification fail
after two checks.
