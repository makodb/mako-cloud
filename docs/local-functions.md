# Serving edge functions locally

`mako functions serve` runs a function in the same pinned Supabase Edge Runtime
release used by Mako's hosted runtime. Docker or Podman must be available; the
CLI starts the image by immutable digest and mounts both the function and Mako
main worker read-only.

JWT verification is on by default:

```bash
mako functions serve ./functions/hello-world \
  --project-id prj_abcdefgh \
  --environment-id env_abcdefgh \
  --api-url http://host.docker.internal:8787 \
  --jwks-file ./.local/jwks.json \
  --jwt-issuer http://localhost:8787/auth/v1 \
  --jwt-audience mako-app \
  --env-file ./.local/function.env \
  --secret-file ./.local/function.secrets
```

Invoke the function through its stable route:

```text
http://127.0.0.1:9000/prj_abcdefgh/functions/v1/hello-world
```

The CLI accepts `KEY=VALUE` files. `--env-file` is for ordinary configuration;
`--secret-file` identifies sensitive values. Only names explicitly present in
those files are mapped into the user worker. Secret values are supplied through
the child process environment, never placed in container arguments or dry-run
output. Runtime stdout and stderr redact exact active secret values before they
reach the terminal. Do not commit either local file when it contains credentials.

Use `--no-verify-jwt` only for a function configured as public. If a public
request supplies an `Authorization` header, the runtime still validates it; pass
the complete JWKS, issuer, and audience options if callers may send tokens. An
unverified token is never exposed as caller identity. Local signature and
tenant-claim checks match hosted ingress, but local serving cannot check remote
session revocation or authorization-epoch freshness unless the local identity
service supplying the JWKS is also running.

The function receives the hosted function-owned path (the public prefix is
removed), query string, standard method, headers, body stream, and request/trace
correlation headers. TypeScript, JavaScript, Fetch APIs, supported npm imports,
WebAssembly, outbound `fetch`, and streaming responses come from the pinned
Deno-compatible runtime. Regional routing, hosted quotas, and production egress
network enforcement are infrastructure differences; local resource limits are
still applied by the user worker. `--wall-time-ms` can lower the per-invocation
wall limit from its 300000 millisecond default and cannot raise it above that
hosted-compatible ceiling. The outer local runtime keeps a fixed shutdown grace
period beyond that limit so it can return the supervised function error instead
of racing the worker with an unrelated runtime-unavailable response.

Use `--dry-run` to inspect the redacted container command. Stop the command and
run it again after changing environment or secret files; source modules are
reloaded according to the pinned runtime's worker lifecycle.
