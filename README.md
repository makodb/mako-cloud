# Mako Cloud

Mako Cloud is an RxDB-native application backend with project authentication,
document-level policies, Supabase-style edge functions, and a cloud management
plane. Production state is stored in exclusively owned local RocksDB databases;
MongoDB and SQL compatibility are outside the MVP.

Start with the [documentation index](docs/README.md). For a local setup, follow
[local development](docs/local-development.md) and then run the
[local-first RxDB example](examples/local-first/README.md).

The current production storage topology is single-node. It does not provide
automatic failover, active-active writes, a shared RocksDB directory, or
horizontal scaling of one database. See the
[deployment guide](docs/deployment.md) and
[production operations guide](docs/production-rocksdb-operations.md) before
evaluating a release.
