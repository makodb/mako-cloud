# Mako Cloud documentation

Two books cover the platform. Read the one written for you.

| Book | For | Covers |
| --- | --- | --- |
| [**The User Book**](user-book.md) | Developers building applications **on** Mako Cloud | Concepts and getting started, the console and the `mako` CLI, teams and projects, collections and indexes, document policies, the document and replication protocols, application authentication and sign-in providers, the `@mako-cloud/rxdb` client, file storage, edge and scheduled functions, webhooks, mail, allowed origins, custom domains, user administration, the data workspace, observability, plans and billing, the API and SDKs, the sample applications, troubleshooting, limits |
| [**The Dev Book**](dev-book.md) | Contributors and operators changing Mako Cloud **itself** | Repository and toolchain, architecture and every crate, storage engines and their contracts, local development, the configuration reference, testing and qualification, the spec-driven workflow, conventions, the threat model and security qualification, deployment and the public beta VM, operations, runbooks, release engineering, the sample applications as gates, the design system |

`npm run build:user-book-docx` renders the User Book as a Word document (`dist/user-book.docx`) for readers who want it offline or printed.

The OpenAPI document ([`api/openapi/mako-cloud-v1.yaml`](../api/openapi/mako-cloud-v1.yaml)) is the authoritative public wire contract, and the checked Rust and TypeScript types are authoritative for internal boundaries. Run `npm run validate:docs` after changing this index or either book.

## Machine-checked companions

These files are read by validators and release tooling rather than by people, and stay separate from the books:

- [Requirements traceability](requirements-traceability.md) — every spec scenario mapped to its primary automated test (`npm run validate:traceability`)
- [`release-gates.json`](release-gates.json) — the release-gate decisions, thresholds, blockers, and bindings (`npm run validate:release-gates`)
- [`rollback-qualification.json`](rollback-qualification.json) — the six rollback drills (`npm run validate:rollback`)
- [`production-rocksdb-qualification.json`](production-rocksdb-qualification.json) and [`performance-baseline.json`](performance-baseline.json) — the storage qualification and the component baseline (`npm run validate:production-release`, `validate:performance`)
- [`evidence/`](evidence/) — machine-readable qualification and deployment evidence, indexed in the Dev Book's [evidence appendix](dev-book.md#appendix-evidence-files)

Qualification reports describe the host and scope they exercised. A passing local report does not silently qualify a different cloud storage class, container runtime, region, or cost model. The reference RxDB application has its own [README](../examples/local-first/README.md).
