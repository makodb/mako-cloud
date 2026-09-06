#!/usr/bin/env node

import { readdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const pinPath = resolve(root, "infra/public-beta/container-images.json");
const quadletRoot = resolve(root, "infra/ansible/roles/dependencies/files/quadlet");
const pins = JSON.parse(await readFile(pinPath, "utf8"));
const runtimePin = JSON.parse(
  await readFile(resolve(root, "infra/edge-runtime/runtime-pin.json"), "utf8"),
);

assert(pins.schemaVersion === 1, "container pin schemaVersion must be 1");
assert(pins.environment === "public-beta", "container pins must be beta-only");
assert(pins.architecture === "linux/amd64", "container pins must target the beta VM");
assert(pins.images?.length === 6, "exactly six public-beta images must be pinned");

const expectedFiles = new Map([
  ["mailpit", "mako-mailpit.container"],
  ["object-store", "mako-object-store.container"],
  ["otel-collector", "mako-otel-collector.container"],
  ["prometheus", "mako-prometheus.container"],
  ["grafana", "mako-grafana.container"],
  ["edge-runtime", "mako-edge-runtime.container"],
]);
const components = new Set();
for (const pin of pins.images) {
  assert(expectedFiles.has(pin.component), `unexpected container component: ${pin.component}`);
  assert(!components.has(pin.component), `duplicate container component: ${pin.component}`);
  components.add(pin.component);
  assert(
    /^docker\.io\/.+@sha256:[0-9a-f]{64}$/u.test(pin.image),
    `${pin.component} does not use an immutable OCI digest`,
  );
  assert(
    /^sha256:[0-9a-f]{64}$/u.test(pin.platformImageId),
    `${pin.component} does not pin its platform image identity`,
  );

  const source = await readFile(resolve(quadletRoot, expectedFiles.get(pin.component)), "utf8");
  assert(source.includes(`Image=${pin.image}\n`), `${pin.component} Quadlet changed image`);
  assert(source.includes("Pull=never\n"), `${pin.component} may pull a moving image`);
  if (pin.component === "prometheus") {
    assert(source.includes("Network=host\n"), "Prometheus cannot scrape private host metrics");
    assert(
      source.includes("--web.listen-address=127.0.0.1:9090\n"),
      "host-networked Prometheus is not bound to loopback",
    );
  } else if (pin.component === "edge-runtime") {
    // The runtime is the one container that has to reach the host's loopback:
    // the platform API it hands every worker as MAKO_API_URL binds there. On
    // the private bridge `host.containers.internal` is the bridge gateway,
    // where nothing listens, so every SDK call a deployed function made was
    // refused -- and this assertion, requiring the bridge, is what held that
    // in place (finding #28). Since the egress allowlist the container may
    // also reach the outside world, because a deployment may declare external
    // hosts and a loopback-bound container would hollow that grant out; what
    // confines *tenant* code is the per-worker sandbox, whose allow_net holds
    // exactly the API origin plus the declared hosts (finding #36). So the
    // line must grant host loopback, keep IPv6 off, and bind outbound traffic
    // nowhere in particular -- for a while this check still demanded the old
    // loopback binding and had been failing unnoticed (finding #47).
    assert(
      source.includes("Network=slirp4netns:allow_host_loopback=true,enable_ipv6=false\n"),
      "the edge runtime cannot reach the platform API, or its network line changed shape",
    );
    assert(
      source.includes("--add-host=host.containers.internal:10.0.2.2"),
      "the edge runtime's API hostname does not name the host of its own network",
    );
    assert(
      source.includes("Environment=MAKO_API_URL=http://host.containers.internal:8080\n"),
      "the edge runtime is given an API origin its workers' SDK would refuse",
    );
  } else {
    assert(
      source.includes("Network=mako-cloud.network\n"),
      `${pin.component} left the private network`,
    );
  }
  assert(source.includes("ReadOnly=true\n"), `${pin.component} root filesystem is writable`);
  assert(source.includes("NoNewPrivileges=true\n"), `${pin.component} can gain privileges`);
  assert(source.includes("DropCapability=all\n"), `${pin.component} retains capabilities`);
  const ports = [...source.matchAll(/^PublishPort=(.+)$/gmu)].map((match) => match[1]);
  assert(
    ports.length > 0 || pin.component === "prometheus",
    `${pin.component} has no explicit host binding`,
  );
  for (const port of ports) {
    assert(port.startsWith("127.0.0.1:"), `${pin.component} publishes a non-loopback listener`);
  }
}

const edge = pins.images.find((pin) => pin.component === "edge-runtime");
assert(
  edge.image === `${runtimePin.imageRepository}@${runtimePin.imageDigest}`,
  "the deployed edge runtime differs from the embedded runtime pin",
);
const edgeQuadlet = await readFile(resolve(quadletRoot, "mako-edge-runtime.container"), "utf8");
assert(
  edgeQuadlet.includes("Exec=start --policy per_request "),
  "edge runtime must reset wall-clock and CPU limits for each invocation",
);
assert(
  edgeQuadlet.includes("PublishPort=127.0.0.1:9001:9000\n"),
  "the runtime supervisor is not published on loopback",
);
assert(
  edgeQuadlet.includes("Volume=mako-edge-runtime.volume:/var/lib/mako-runtime-supervisor\n"),
  "the runtime supervisor has no durable encrypted-state volume",
);
assert(
  edgeQuadlet.includes("Tmpfs=/var/lib/mako-runtime-workers:rw,noexec,nosuid,nodev,size=256m\n"),
  "the runtime supervisor plaintext worker area is not memory-only",
);
assert(
  edgeQuadlet.includes(
    "EnvironmentFile=/home/mako-runtime/.config/mako-cloud/runtime-supervisor.env\n",
  ),
  "the runtime supervisor has no protected credential source",
);
const supervisor = await readFile(resolve(root, "packages/cli/runtime/main/supervisor.ts"), "utf8");
const runtimeMain = await readFile(resolve(root, "packages/cli/runtime/main/index.ts"), "utf8");
assert(
  runtimeMain.includes('pathname.startsWith("/_mako/runtime/")'),
  "the single pinned-runtime listener does not dispatch authenticated supervisor routes",
);
for (const path of [
  "health",
  "deployments/load",
  "deployments/probe",
  "deployments/test",
  "deployments/logs",
  "deployments/retire",
  "functions/retire",
]) {
  assert(supervisor.includes(`/_mako/runtime/v1/${path}`), `runtime supervisor omits ${path}`);
}
assert(supervisor.includes("crypto.subtle.encrypt"), "runtime state is not encrypted");
assert(supervisor.includes("crypto.subtle.decrypt"), "runtime state cannot recover after restart");

const grafana = await readFile(resolve(quadletRoot, "mako-grafana.container"), "utf8");
assert(
  grafana.includes("Environment=GF_AUTH_ANONYMOUS_ENABLED=false\n"),
  "Grafana anonymous access is not disabled",
);
assert(!grafana.includes("GF_AUTH_ANONYMOUS_ENABLED=true"), "Grafana enables anonymous access");
assert(
  grafana.includes("Environment=GF_USERS_ALLOW_SIGN_UP=false\n"),
  "Grafana permits anonymous sign-up",
);
assert(
  grafana.includes("ConditionPathExists=/home/mako-runtime/.config/mako-cloud/grafana.env"),
  "Grafana does not fail closed without its protected credential file",
);

const network = await readFile(resolve(quadletRoot, "mako-cloud.network"), "utf8");
assert(network.includes("Internal=true\n"), "dependency network permits direct external egress");

const target = await readFile(
  resolve(root, "infra/ansible/roles/dependencies/files/mako-dependencies.target"),
  "utf8",
);
for (const file of expectedFiles.values()) {
  const service = file.replace(/\.container$/u, ".service");
  assert(target.includes(`Wants=${service}\n`), `dependency target omits ${service}`);
}

const actualFiles = (await readdir(quadletRoot)).filter((name) => name.endsWith(".container"));
assert(actualFiles.length === expectedFiles.size, "unvalidated public-beta containers exist");
console.log(
  `validated ${pins.images.length} immutable rootless public-beta containers; ` +
    "all published listeners are loopback-only and Grafana anonymous access is disabled",
);

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
