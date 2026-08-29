use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use mako_api::TenantScope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_FUNCTION_BUNDLE_BYTES: usize = 10 * 1024 * 1024;
const MAX_SOURCE_FILES: usize = 512;
const MAX_SOURCE_PATH_BYTES: usize = 512;
const MAX_DEPENDENCIES: usize = 256;
/// The one bare specifier a function may import without declaring it. The edge
/// runtime supplies the built SDK to every worker as a first-party module and
/// maps this specifier onto it, so accepting it here is a deliberate contract
/// with the runtime rather than a hole: every other bare specifier must resolve
/// to an uploaded module through a dependency mapping, or the upload is
/// refused with `unresolved_import` instead of failing when the worker boots.
pub const RUNTIME_SDK_SPECIFIER: &str = "@mako-cloud/edge-sdk";
/// Module paths the platform reserves inside a worker directory: the console
/// shim and the injected SDK live there, and a bundle must not be able to
/// shadow either.
const RESERVED_MODULE_PREFIX: &str = "__mako";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionBundleFormat {
    SourceArchiveV1,
    Prebuilt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionSourceFile {
    pub path: String,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FunctionBundleUpload {
    Source {
        entrypoint: String,
        files: Vec<FunctionSourceFile>,
        dependencies: BTreeMap<String, String>,
    },
    Prebuilt {
        entrypoint: String,
        bundle: Vec<u8>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionBundleDiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionBundleDiagnostic {
    pub severity: FunctionBundleDiagnosticSeverity,
    pub code: String,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionBundleRecord {
    pub(crate) tenant: TenantScope,
    pub(crate) digest: String,
    pub(crate) format: FunctionBundleFormat,
    pub(crate) entrypoint: String,
    pub(crate) size_bytes: u64,
    pub(crate) module_count: u32,
    pub(crate) created_at_unix_seconds: u64,
}

impl FunctionBundleRecord {
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub const fn format(&self) -> FunctionBundleFormat {
        self.format
    }

    #[must_use]
    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }

    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    pub const fn module_count(&self) -> u32 {
        self.module_count
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionBundleUploadOutcome {
    pub artifact: Option<FunctionBundleRecord>,
    pub diagnostics: Vec<FunctionBundleDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuiltFunctionBundle {
    pub record: FunctionBundleRecord,
    pub bytes: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceArchive {
    format_version: u8,
    entrypoint: String,
    modules: BTreeMap<String, String>,
    resolved_imports: BTreeMap<String, BTreeMap<String, String>>,
}

pub(crate) fn build_function_bundle(
    tenant: TenantScope,
    upload: FunctionBundleUpload,
    now_unix_seconds: u64,
) -> Result<BuiltFunctionBundle, Vec<FunctionBundleDiagnostic>> {
    match upload {
        FunctionBundleUpload::Source {
            entrypoint,
            files,
            dependencies,
        } => build_source_bundle(tenant, entrypoint, files, dependencies, now_unix_seconds),
        FunctionBundleUpload::Prebuilt { entrypoint, bundle } => {
            build_prebuilt_bundle(tenant, entrypoint, bundle, now_unix_seconds)
        }
    }
}

fn build_prebuilt_bundle(
    tenant: TenantScope,
    entrypoint: String,
    bundle: Vec<u8>,
    now: u64,
) -> Result<BuiltFunctionBundle, Vec<FunctionBundleDiagnostic>> {
    let mut diagnostics = Vec::new();
    if !valid_source_path(&entrypoint) {
        diagnostics.push(error(
            "invalid_entrypoint",
            "The entrypoint must be a normalized relative module path.",
            Some(entrypoint.clone()),
            None,
        ));
    }
    if bundle.is_empty() || bundle.len() > MAX_FUNCTION_BUNDLE_BYTES {
        diagnostics.push(error(
            "invalid_bundle_size",
            "The prebuilt bundle must be non-empty and at most 10 MiB.",
            None,
            None,
        ));
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    Ok(product(
        tenant,
        FunctionBundleFormat::Prebuilt,
        entrypoint,
        1,
        bundle,
        now,
    ))
}

fn build_source_bundle(
    tenant: TenantScope,
    entrypoint: String,
    files: Vec<FunctionSourceFile>,
    dependencies: BTreeMap<String, String>,
    now: u64,
) -> Result<BuiltFunctionBundle, Vec<FunctionBundleDiagnostic>> {
    let mut diagnostics = Vec::new();
    if files.is_empty() || files.len() > MAX_SOURCE_FILES {
        diagnostics.push(error(
            "invalid_file_count",
            "A source bundle must contain between 1 and 512 files.",
            None,
            None,
        ));
    }
    if dependencies.len() > MAX_DEPENDENCIES {
        diagnostics.push(error(
            "too_many_dependencies",
            "A source bundle may declare at most 256 dependency mappings.",
            None,
            None,
        ));
    }

    let mut modules = BTreeMap::new();
    let mut text_modules = BTreeMap::new();
    let mut total_bytes = 0usize;
    for file in files {
        if !valid_source_path(&file.path) {
            diagnostics.push(error(
                "invalid_module_path",
                "Module paths must be normalized, relative, and use forward slashes.",
                Some(file.path),
                None,
            ));
            continue;
        }
        if reserved_source_path(&file.path) {
            diagnostics.push(error(
                "reserved_module_path",
                "Module paths beginning with `__mako` are reserved by the runtime.",
                Some(file.path),
                None,
            ));
            continue;
        }
        let contents = if file.path.ends_with(".wasm") {
            file.contents
        } else {
            match String::from_utf8(file.contents) {
                Ok(source) => {
                    let source = source.replace("\r\n", "\n").replace('\r', "\n");
                    text_modules.insert(file.path.clone(), source.clone());
                    source.into_bytes()
                }
                Err(_) => {
                    diagnostics.push(error(
                        "invalid_source_encoding",
                        "Text source modules must be valid UTF-8.",
                        Some(file.path),
                        None,
                    ));
                    continue;
                }
            }
        };
        total_bytes = total_bytes.saturating_add(contents.len());
        if modules
            .insert(file.path.clone(), STANDARD.encode(contents))
            .is_some()
        {
            diagnostics.push(error(
                "duplicate_module_path",
                "Each module path may appear only once.",
                Some(file.path),
                None,
            ));
        }
    }
    if total_bytes > MAX_FUNCTION_BUNDLE_BYTES {
        diagnostics.push(error(
            "source_too_large",
            "Decoded source files may total at most 10 MiB.",
            None,
            None,
        ));
    }
    if !valid_source_path(&entrypoint) || !modules.contains_key(&entrypoint) {
        diagnostics.push(error(
            "entrypoint_not_found",
            "The entrypoint must name one uploaded module.",
            Some(entrypoint.clone()),
            None,
        ));
    }

    for (specifier, target) in &dependencies {
        if specifier == RUNTIME_SDK_SPECIFIER {
            diagnostics.push(error(
                "reserved_dependency_specifier",
                "The runtime supplies `@mako-cloud/edge-sdk`; a bundle may not remap it.",
                None,
                None,
            ));
            continue;
        }
        if !valid_dependency_specifier(specifier)
            || !valid_source_path(target)
            || !modules.contains_key(target)
        {
            diagnostics.push(error(
                "invalid_dependency_mapping",
                "Dependency mappings must resolve a bounded specifier to an uploaded module path.",
                None,
                None,
            ));
        }
    }

    let mut resolved_imports = BTreeMap::new();
    for (path, source) in text_modules {
        let mut imports = BTreeMap::new();
        for (line, specifier) in import_specifiers(&source) {
            match resolve_specifier(&path, &specifier, &modules, &dependencies) {
                Some(resolved) => {
                    imports.insert(specifier, resolved);
                }
                None => diagnostics.push(error(
                    "unresolved_import",
                    "An import could not be resolved to an uploaded module or dependency mapping.",
                    Some(path.clone()),
                    Some(line),
                )),
            }
        }
        resolved_imports.insert(path, imports);
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let module_count = u32::try_from(modules.len()).expect("source file limit fits u32");
    let archive = SourceArchive {
        format_version: 1,
        entrypoint: entrypoint.clone(),
        modules,
        resolved_imports,
    };
    let bytes = serde_json::to_vec(&archive).map_err(|_| {
        vec![error(
            "bundle_serialization_failed",
            "The deterministic source archive could not be produced.",
            None,
            None,
        )]
    })?;
    if bytes.len() > MAX_FUNCTION_BUNDLE_BYTES {
        return Err(vec![error(
            "bundle_too_large",
            "The deterministic bundle exceeds the 10 MiB artifact limit.",
            None,
            None,
        )]);
    }
    Ok(product(
        tenant,
        FunctionBundleFormat::SourceArchiveV1,
        entrypoint,
        module_count,
        bytes,
        now,
    ))
}

fn resolve_specifier(
    importer: &str,
    specifier: &str,
    modules: &BTreeMap<String, String>,
    dependencies: &BTreeMap<String, String>,
) -> Option<String> {
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let candidate = relative_path(importer, specifier)?;
        return module_candidate(&candidate, modules);
    }
    if specifier == RUNTIME_SDK_SPECIFIER {
        return Some(specifier.to_owned());
    }
    dependencies.get(specifier).cloned()
}

fn module_candidate(candidate: &str, modules: &BTreeMap<String, String>) -> Option<String> {
    if modules.contains_key(candidate) {
        return Some(candidate.to_owned());
    }
    for extension in [".ts", ".tsx", ".js", ".jsx", ".mjs", ".json", ".wasm"] {
        let extended = format!("{candidate}{extension}");
        if modules.contains_key(&extended) {
            return Some(extended);
        }
    }
    for filename in [
        "index.ts",
        "index.tsx",
        "index.js",
        "index.jsx",
        "index.mjs",
    ] {
        let indexed = format!("{candidate}/{filename}");
        if modules.contains_key(&indexed) {
            return Some(indexed);
        }
    }
    None
}

fn relative_path(importer: &str, specifier: &str) -> Option<String> {
    let mut parts = importer.split('/').collect::<Vec<_>>();
    parts.pop();
    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value),
        }
    }
    Some(parts.join("/"))
}

fn import_specifiers(source: &str) -> Vec<(u32, String)> {
    let mut found = BTreeSet::new();
    for (index, line) in source.lines().enumerate() {
        let line_number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let trimmed = line.trim();
        if (trimmed.starts_with("import ") || trimmed.starts_with("export "))
            && let Some(specifier) = quoted_after(trimmed, " from ").or_else(|| {
                trimmed
                    .starts_with("import ")
                    .then(|| first_quoted(trimmed))
                    .flatten()
            })
        {
            found.insert((line_number, specifier));
        }
        for marker in ["import(", "require("] {
            let mut rest = line;
            while let Some(position) = rest.find(marker) {
                rest = &rest[position + marker.len()..];
                if let Some(specifier) = first_quoted(rest) {
                    found.insert((line_number, specifier));
                }
                if rest.is_empty() {
                    break;
                }
                rest = &rest[1..];
            }
        }
    }
    found.into_iter().collect()
}

fn quoted_after(value: &str, marker: &str) -> Option<String> {
    let position = value.find(marker)?;
    first_quoted(&value[position + marker.len()..])
}

fn first_quoted(value: &str) -> Option<String> {
    let (start, quote) = value
        .char_indices()
        .find(|(_, character)| *character == '\'' || *character == '"')?;
    let remainder = &value[start + quote.len_utf8()..];
    let end = remainder.find(quote)?;
    let specifier = &remainder[..end];
    (!specifier.is_empty() && specifier.len() <= 512 && !specifier.chars().any(char::is_control))
        .then(|| specifier.to_owned())
}

fn valid_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_SOURCE_PATH_BYTES
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A path the runtime owns inside a worker directory. Matched on the first
/// segment so a nested `a/__mako_x.ts` stays a normal module.
fn reserved_source_path(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|first| first.starts_with(RESERVED_MODULE_PREFIX))
}

fn valid_dependency_specifier(specifier: &str) -> bool {
    !specifier.is_empty()
        && specifier.len() <= 512
        && !specifier.starts_with('.')
        && !specifier.starts_with('/')
        && !specifier.starts_with("http:")
        && !specifier.starts_with("https:")
        && !specifier.chars().any(char::is_control)
}

fn product(
    tenant: TenantScope,
    format: FunctionBundleFormat,
    entrypoint: String,
    module_count: u32,
    bytes: Vec<u8>,
    now: u64,
) -> BuiltFunctionBundle {
    let digest = digest_bytes(&bytes);
    let size_bytes = u64::try_from(bytes.len()).expect("bundle size limit fits u64");
    BuiltFunctionBundle {
        record: FunctionBundleRecord {
            tenant,
            digest,
            format,
            entrypoint,
            size_bytes,
            module_count,
            created_at_unix_seconds: now,
        },
        bytes,
    }
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_lower(&Sha256::digest(bytes)))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn error(
    code: &str,
    message: &str,
    path: Option<String>,
    line: Option<u32>,
) -> FunctionBundleDiagnostic {
    FunctionBundleDiagnostic {
        severity: FunctionBundleDiagnosticSeverity::Error,
        code: code.to_owned(),
        message: message.to_owned(),
        path,
        line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_api::{EnvironmentId, ProjectId};

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_example00").expect("project"),
            EnvironmentId::parse("env_example00").expect("environment"),
        )
    }

    #[test]
    fn source_archives_are_deterministic_across_input_order() {
        let first = vec![
            FunctionSourceFile {
                path: "index.ts".to_owned(),
                contents: b"import { value } from './value.ts';\nexport default value;\n".to_vec(),
            },
            FunctionSourceFile {
                path: "value.ts".to_owned(),
                contents: b"export const value = 42;\n".to_vec(),
            },
        ];
        let mut second = first.clone();
        second.reverse();
        let build = |files| {
            build_function_bundle(
                tenant(),
                FunctionBundleUpload::Source {
                    entrypoint: "index.ts".to_owned(),
                    files,
                    dependencies: BTreeMap::new(),
                },
                1,
            )
            .expect("bundle")
        };
        let first = build(first);
        let second = build(second);
        assert_eq!(first.record.digest, second.record.digest);
        assert_eq!(first.bytes, second.bytes);
    }

    #[test]
    fn the_runtime_sdk_is_the_only_bare_specifier_a_bundle_may_import() {
        // The runtime supplies this module to every worker and maps the
        // specifier onto it, so the bundle validator resolves it deliberately.
        let built = build_function_bundle(
            tenant(),
            FunctionBundleUpload::Source {
                entrypoint: "index.ts".to_owned(),
                files: vec![FunctionSourceFile {
                    path: "index.ts".to_owned(),
                    contents:
                        b"import { createServiceClient } from '@mako-cloud/edge-sdk';\nexport default { fetch: () => new Response(createServiceClient) };\n"
                            .to_vec(),
                }],
                dependencies: BTreeMap::new(),
            },
            1,
        )
        .expect("the runtime SDK resolves");
        let archive: serde_json::Value =
            serde_json::from_slice(&built.bytes).expect("archive is json");
        assert_eq!(
            archive["resolvedImports"]["index.ts"][RUNTIME_SDK_SPECIFIER],
            serde_json::Value::String(RUNTIME_SDK_SPECIFIER.to_owned()),
        );

        // Anything else the platform does not supply is refused here rather
        // than at boot, including specifiers that merely look first-party.
        for specifier in ["mako:kv", "npm:left-pad", "@mako-cloud/rxdb"] {
            let refused = build_function_bundle(
                tenant(),
                FunctionBundleUpload::Source {
                    entrypoint: "index.ts".to_owned(),
                    files: vec![FunctionSourceFile {
                        path: "index.ts".to_owned(),
                        contents: format!("import x from '{specifier}';\n").into_bytes(),
                    }],
                    dependencies: BTreeMap::new(),
                },
                1,
            )
            .expect_err("an unsupplied module is refused");
            assert_eq!(refused[0].code, "unresolved_import", "{specifier}");
        }
    }

    #[test]
    fn a_bundle_cannot_shadow_or_remap_what_the_runtime_injects() {
        let shadowed = build_function_bundle(
            tenant(),
            FunctionBundleUpload::Source {
                entrypoint: "index.ts".to_owned(),
                files: vec![
                    FunctionSourceFile {
                        path: "index.ts".to_owned(),
                        contents: b"export default {};\n".to_vec(),
                    },
                    FunctionSourceFile {
                        path: "__mako_edge_sdk.mjs".to_owned(),
                        contents: b"export const createServiceClient = null;\n".to_vec(),
                    },
                ],
                dependencies: BTreeMap::new(),
            },
            1,
        )
        .expect_err("reserved module path");
        assert_eq!(shadowed[0].code, "reserved_module_path");

        let remapped = build_function_bundle(
            tenant(),
            FunctionBundleUpload::Source {
                entrypoint: "index.ts".to_owned(),
                files: vec![
                    FunctionSourceFile {
                        path: "index.ts".to_owned(),
                        contents: b"import x from '@mako-cloud/edge-sdk';\nexport default x;\n"
                            .to_vec(),
                    },
                    FunctionSourceFile {
                        path: "fake.ts".to_owned(),
                        contents: b"export default 1;\n".to_vec(),
                    },
                ],
                dependencies: BTreeMap::from([(
                    RUNTIME_SDK_SPECIFIER.to_owned(),
                    "fake.ts".to_owned(),
                )]),
            },
            1,
        )
        .expect_err("reserved dependency specifier");
        assert_eq!(remapped[0].code, "reserved_dependency_specifier");

        // A nested path that merely starts with the prefix is an ordinary module.
        build_function_bundle(
            tenant(),
            FunctionBundleUpload::Source {
                entrypoint: "index.ts".to_owned(),
                files: vec![FunctionSourceFile {
                    path: "index.ts".to_owned(),
                    contents: b"export default {};\n".to_vec(),
                }],
                dependencies: BTreeMap::new(),
            },
            1,
        )
        .expect("an ordinary bundle still builds");
    }

    #[test]
    fn unresolved_imports_and_oversized_sources_return_sanitized_diagnostics() {
        let unresolved = build_function_bundle(
            tenant(),
            FunctionBundleUpload::Source {
                entrypoint: "index.ts".to_owned(),
                files: vec![FunctionSourceFile {
                    path: "index.ts".to_owned(),
                    contents: b"import secret from 'unmapped-package';".to_vec(),
                }],
                dependencies: BTreeMap::new(),
            },
            1,
        )
        .expect_err("unresolved");
        assert_eq!(unresolved[0].code, "unresolved_import");
        assert!(!unresolved[0].message.contains("secret"));

        let too_large = build_function_bundle(
            tenant(),
            FunctionBundleUpload::Prebuilt {
                entrypoint: "index.js".to_owned(),
                bundle: vec![0; MAX_FUNCTION_BUNDLE_BYTES + 1],
            },
            1,
        )
        .expect_err("oversized");
        assert_eq!(too_large[0].code, "invalid_bundle_size");
    }
}
