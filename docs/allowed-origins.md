# Allowed origins (CORS)

A browser refuses to hand a cross-origin response to the page that asked for
it unless the server names that page's origin back. An environment carries
the list of origins that may be named: the browser applications allowed to
call its application API.

The list belongs to the **environment**, not to a hostname, so it applies
everywhere that environment's API is served — on the platform's own
hostname, `https://<platform>/v1/projects/{projectId}/environments/{environmentId}/…`,
and on every [custom domain](custom-domains.md) the environment is verified
for. An application therefore needs no domain of its own before its pages
can call its own API.

## Reading and setting the list

```
GET  /v1/projects/{projectId}/environments/{environmentId}/allowed-origins
PUT  /v1/projects/{projectId}/environments/{environmentId}/allowed-origins
```

Both carry the same shape, and `PUT` replaces the list whole (with an
idempotency key):

```json
{ "allowedOrigins": ["https://app.example.com", "http://127.0.0.1:5173"] }
```

Any member of the project's team reads the list; a member who may mutate
projects (developer, administrator, owner) sets it. Both are recorded in the
control audit log as `allowed_origins_read` and `allowed_origins_update`.

An environment that has never set a list allows no origin, which is what
every environment did before this existed. An empty list means the same and
is how cross-origin access is withdrawn.

## What an origin may be

An origin is **exact**: `scheme://host` or `scheme://host:port`, with a
lowercase host and nothing else — no path, no trailing slash, no wildcard,
no userinfo, no query or fragment. `https` is required except for a loopback
host (`localhost`, a name under it, or an address in `127.0.0.0/8`), which
may use `http` so a development server on the developer's own machine can be
listed. Origins must be unique, at most 262 characters each, and an
environment lists **at most 16**. Anything else is refused with
`400 invalid_request` naming the rule it broke.

The control plane stores the list and installs it into the data plane, which
answers browsers from it; the edge gateway receives it with the route it
resolves for a function. Both refuse an origin they could not match byte for
byte, so a value that could not be compared exactly never becomes an echoed
header.

## What the platform emits, and when

Cross-origin headers are emitted only on a request that carries an `Origin`
the addressed environment lists, and only on the routes a browser
application calls:

| Request | Answer |
| --- | --- |
| `OPTIONS` on an application route, listed `Origin` | `204` with `Access-Control-Allow-Origin: <the origin>`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers`, `Access-Control-Max-Age: 600`, `Vary: Origin` |
| Any other method on an application route, listed `Origin` | the route's own answer, plus `Access-Control-Allow-Origin: <the origin>`, `Access-Control-Expose-Headers`, `Vary: Origin` |
| Unlisted or absent `Origin` | the route's own answer, unchanged — an `OPTIONS` routes as it always did |
| Anything on a management, operator, developer-workspace, or `/service/` route | the route's own answer, unchanged, **whatever** the environment allows |

The emitted values are the platform's, not the caller's:

- `Access-Control-Allow-Methods: GET, POST, PUT, PATCH, DELETE, OPTIONS`
- `Access-Control-Allow-Headers: authorization, content-type, x-mako-key,
  idempotency-key, if-none-match, if-match`
- `Access-Control-Expose-Headers: etag, x-mako-request-id, content-type`
- `Access-Control-Max-Age: 600`

`Access-Control-Allow-Credentials` is never sent and `*` is never sent: an
application carries its session in the `Authorization` header and its public
key in `X-Mako-Key`, so cookies are not part of the exchange. A preflight
from a listed origin is answered by the platform before the request is
routed, so it costs nothing on the route. A failure is labelled like a
success: an application that cannot read a `401` cannot react to it.

**Application routes** are the ones an application calls with its own
session or public key: `…/auth/…`, `…/collections/…` (documents and
replication), and `…/storage/…`. The developer workspace's explorer, the
`/service/` routes a service credential uses from a server, the management
and operator APIs, and the private internal protocol are never answered
cross-origin — the control plane installs no cross-origin handling at all.

Edge functions are covered too, on both shapes: `/{projectRef}/functions/v1/{name}`
on the platform hostname and `/functions/v1/{name}` on a custom domain. The
gateway answers a preflight from a listed origin itself; an `OPTIONS` that is
not such a preflight still reaches the function, which may answer it however
it likes.

## The topology this is for

The platform serves an API, not static files. A browser application is
served from **its own hostname** — a static host, or `http://127.0.0.1:5173`
while it is being written — and calls the project's API on the platform
hostname or on the project's custom domain. That is two origins, so every
call is cross-origin, and it works exactly when the application's origin is
in the environment's list.

Two things worth stating plainly: an origin that is not listed is refused by
the **browser**, not by the platform — the request may still reach the API
and is authorized on its own merits, so the allowlist is a browser-safety
mechanism and never an authorization one; and the list is per environment,
so an application's development origin can be listed in a development
environment without ever being listed in production.
