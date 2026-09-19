# Mako Cloud

Mako Cloud is an RxDB-native application backend with project authentication,
document-level policies, RxDB replication, file storage, Supabase-style edge
functions, and a cloud management plane. Production state is stored in
exclusively owned local databases on a single node; MongoDB and SQL
compatibility are outside the MVP.

Two books document it:

- [**The User Book**](docs/user-book.md) — for developers building applications
  on Mako Cloud: concepts, getting started, the console and CLI, every
  capability, the API and SDKs, troubleshooting.
- [**The Dev Book**](docs/dev-book.md) — for contributors and operators of the
  platform: architecture, storage, local development, configuration, testing,
  security, deployment, operations, runbooks, and release engineering.

For a local setup, follow the Dev Book's
[local development](docs/dev-book.md#local-development) chapter and then run the
[local-first RxDB example](examples/local-first/README.md).

The current production storage topology is single-node. It does not provide
automatic failover, active-active writes, a shared database directory, or
horizontal scaling of one database. See the Dev Book's
[deployment](docs/dev-book.md#deployment) and
[operations](docs/dev-book.md#operations) chapters before evaluating a release.
