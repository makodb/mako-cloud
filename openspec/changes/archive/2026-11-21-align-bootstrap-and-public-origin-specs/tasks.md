## 1. Specifications

- [x] 1.1 `operations/local-bootstrap-and-smoke`: reword "Local tenant bootstrap" and its scenario "Bootstrap runs a second time".
- [x] 1.2 `operations/public-beta-environment`: reword "Public traffic uses trusted HTTPS".

## 2. Evidence that the code already meets them

- [x] 2.1 `crates/mako-local-bootstrap/src/main.rs` rotates an existing public credential under a new credential id and reports the new key; it deploys the sample functions only when the runtime supervisor is reachable.
- [x] 2.2 `infra/ansible/roles/runtime/templates/Caddyfile.j2` serves the platform hostname and a separate on-demand-TLS site for custom domains, whose certificates are gated by the control plane's ask endpoint.

## 3. Validation

- [x] 3.1 Run `openspec validate align-bootstrap-and-public-origin-specs --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
