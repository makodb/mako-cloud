## Why

The bare `mako` executable is easy to confuse with unrelated Mako database and developer tools. While the product is still in alpha, the command should use the full Mako Cloud name before users and automation depend on the ambiguous binary.

## What Changes

- **BREAKING**: Rename the installed developer CLI executable from `mako` to `mako-cloud` without retaining a `mako` alias.
- Make generated help, validation errors, recovery instructions, and progress output name `mako-cloud`.
- Update user and developer documentation, examples, executable scripts, tests, and the current developer CLI specification to use the new command.
- Add a repository check that prevents the old executable name from returning in current CLI code and documentation.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cloud/developer-cli`: The developer CLI is invoked exclusively through the `mako-cloud` executable.

## Impact

The change affects the `@mako-cloud/cli` package manifest, CLI output strings, CLI tests, repository examples and scripts that invoke the developer command, both documentation books, and the developer CLI specification. It does not rename internal Rust crates, services, configuration variables, protocol headers, storage paths, or the `@mako-cloud/*` package scope.
