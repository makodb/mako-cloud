## 1. Specification

- [x] 1.1 Add the requirement "Public admission has four explicit modes" to `operations/public-beta-environment`, with its four scenarios.
- [x] 1.2 Reword the scenario "A critical readiness dependency fails" under "Operations and teardown are observable and recoverable" so that it says what each mode does.

## 2. Evidence that the deployment already meets it

- [x] 2.1 Confirm in `infra/ansible/roles/runtime/tasks/main.yml` that convergence asserts the mode is one of the four, refuses an enabled reverse proxy in `disabled`, refuses `pre_gate` without a management or tester network, refuses `approved_beta` unless the approval equals `APPROVE_PUBLIC_BETA:` plus the selected release digest, renders all four configurations, and selects `pre_gate` first when the mode is `risk_accepted_preview`.
- [x] 2.2 Confirm in `infra/ansible/roles/runtime/templates/Caddyfile.j2` that the disabled configuration answers 503 and the pre-gate configuration refuses sources outside the configured networks, and in `infra/ansible/playbooks/public-beta.yml` that the guard timer is enabled only in `risk_accepted_preview`.

## 3. Validation

- [x] 3.1 Run `openspec validate name-public-admission-modes --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability`, `npm run validate:docs`, `npm run validate:public-beta-caddy` and `npm run test:public-beta-preview`.
