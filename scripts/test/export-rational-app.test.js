// The standalone export carries the design system: its sources under
// `src/kit`, its package name resolved there, and its dependencies spelled
// out, so the exported repository builds without the workspace.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** A throwaway application checkout: an empty repository with the files the export preserves. */
function throwawayCheckout() {
  const root = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), "rational-export-"));
  execFileSync("git", ["init", "--quiet", root]);
  writeFileSync(
    join(root, "rational.config.example.json"),
    JSON.stringify({
      endpoint: "https://cloud-test.makodb.com",
      projectId: "prj_replace_me",
      environmentId: "env_replace_me",
      publicProjectKey: "mako_pk.replace_me",
      signIn: { providers: [], magicLinks: true },
    }),
  );
  mkdirSync(join(root, ".github/workflows"), { recursive: true });
  writeFileSync(join(root, ".github/workflows/pages.yml"), "with:\n  path: web-dist\n");
  writeFileSync(join(root, "README.md"), "# rational\n");
  return root;
}

test("the export vendors the kit and resolves its package name to the vendored copy", () => {
  const checkout = throwawayCheckout();
  try {
    const output = execFileSync(
      process.execPath,
      [join(repositoryRoot, "scripts/export-rational-app.mjs"), "--dir", checkout, "--dry-run"],
      { cwd: repositoryRoot, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
    assert.match(output, /files exported to/);

    // The kit's sources travel, and its stylesheet is reached relatively.
    for (const file of ["src/kit/index.ts", "src/kit/styles.css", "src/kit/charts.tsx"]) {
      assert.ok(existsSync(join(checkout, file)), `${file} was not exported`);
    }
    const appStylesheet = readFileSync(join(checkout, "src/app.css"), "utf8");
    assert.match(appStylesheet, /@import "\.\/kit\/styles\.css";/);
    assert.doesNotMatch(appStylesheet, /@mako-cloud\/ui/);
    assert.match(output, /src\/app\.css: "@mako-cloud\/ui\/styles\.css" -> "\.\/kit\/styles\.css"/);

    // The package name resolves to the copy, for the type checker and for Vite.
    const tsconfig = JSON.parse(readFileSync(join(checkout, "tsconfig.json"), "utf8"));
    assert.deepEqual(tsconfig.compilerOptions.paths, {
      "@mako-cloud/ui": ["./src/kit/index.ts"],
      "@mako-cloud/ui/*": ["./src/kit/*"],
    });
    assert.equal(tsconfig.references, undefined);
    const viteConfig = readFileSync(join(checkout, "vite.config.ts"), "utf8");
    assert.match(viteConfig, /import tailwindcss from "@tailwindcss\/vite";/);
    assert.match(viteConfig, /plugins: \[tailwindcss\(\)\]/);
    assert.match(
      viteConfig,
      /"@mako-cloud\/ui": fileURLToPath\(new URL\("\.\/src\/kit\/index\.ts"/,
    );

    // The kit is not a dependency there; what it depends on is, at the pinned versions.
    const manifest = JSON.parse(readFileSync(join(checkout, "package.json"), "utf8"));
    const kit = JSON.parse(readFileSync(join(repositoryRoot, "packages/ui/package.json"), "utf8"));
    assert.equal(manifest.dependencies["@mako-cloud/ui"], undefined);
    for (const [name, range] of Object.entries(kit.dependencies)) {
      assert.equal(manifest.dependencies[name], range, `${name} is missing or at another version`);
    }
    assert.equal(manifest.dependencies["lucide-react"], kit.dependencies["lucide-react"]);
    assert.ok(manifest.devDependencies.tailwindcss, "tailwindcss is a build dependency");
    assert.ok(
      manifest.devDependencies["@tailwindcss/vite"],
      "the Vite plugin is a build dependency",
    );
    assert.equal(
      Object.keys(manifest.dependencies).join(),
      Object.keys(manifest.dependencies).sort().join(),
    );
  } finally {
    rmSync(checkout, { recursive: true, force: true });
  }
});
