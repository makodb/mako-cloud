import { execFile as execFileCallback } from "node:child_process";
import { Resolver } from "node:dns/promises";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);
const activeNeighborStates = new Set(["reachable", "stale", "delay", "probe", "permanent"]);

export async function collectConflictEvidence({
  fqdn,
  targetAddress,
  runner = hostProbeRunner(),
  dnsResolvers = defaultDnsResolvers(),
} = {}) {
  const [dns, route] = await Promise.all([
    resolveDnsEvidence(fqdn, dnsResolvers),
    runner.route(targetAddress),
  ]);
  const address = await runner.probeAddress(targetAddress, route.device);
  return { dns, address: { ...address, routeDevice: route.device, onLink: route.onLink } };
}

export function evaluateConflicts({ request, discovery, evidence }) {
  const target = request.identity.ipv4Address;
  const fqdn = request.identity.fqdn;
  const vmName = request.identity.vmName;
  const blockers = [];
  const bridges = discovery.networks.filter((network) =>
    ipv4SubnetContains(network.address, network.prefixLength, target),
  );
  if (bridges.length !== 1) {
    blockers.push(
      `expected exactly one active bridge containing ${target}; found ${bridges.length}`,
    );
  }
  const bridge = bridges[0];
  if (bridge?.gateway === null || bridge?.gateway === undefined) {
    blockers.push("the selected bridge has no authoritative gateway");
  }
  if (evidence.address.routeDevice !== bridge?.bridge) {
    blockers.push(
      `route to ${target} uses ${evidence.address.routeDevice}, not the selected bridge ${bridge?.bridge ?? "<none>"}`,
    );
  }
  if (evidence.address.onLink !== true) blockers.push(`${target} is not proven on-link`);

  const expectedA = [target];
  if (!sameSet(evidence.dns.localA, expectedA)) {
    blockers.push(`local DNS for ${fqdn} is ${list(evidence.dns.localA)}, expected only ${target}`);
  }
  for (const answer of evidence.dns.publicAnswers) {
    if (!sameSet(answer.a, expectedA)) {
      blockers.push(
        `${answer.server} resolves ${fqdn} to ${list(answer.a)}, expected only ${target}`,
      );
    }
    if (request.networkPolicy.ipv6Mode === "disabled" && answer.aaaa.length > 0) {
      blockers.push(
        `${answer.server} returns unexpected AAAA records for ${fqdn}: ${list(answer.aaaa)}`,
      );
    }
  }
  if (evidence.dns.errors.length > 0) blockers.push(...evidence.dns.errors);

  const nameMatches = discovery.guests.filter(
    (guest) => guest.name === vmName || guest.config.name === vmName,
  );
  const addressMatches = discovery.guests.filter((guest) => guestHasAddress(guest, target));
  const proposedVmId = nameMatches.length === 1 ? nameMatches[0].vmId : discovery.nextVmId;
  const vmIdMatches = discovery.guests.filter((guest) => guest.vmId === proposedVmId);
  const expectedGuest =
    nameMatches.length === 1 &&
    addressMatches.length === 1 &&
    nameMatches[0].vmId === addressMatches[0].vmId &&
    nameMatches[0].type === "qemu"
      ? nameMatches[0]
      : null;

  if (nameMatches.length > 1) blockers.push(`multiple guests use VM name ${vmName}`);
  if (addressMatches.length > 1) blockers.push(`multiple guests declare address ${target}`);
  if (nameMatches.length === 1 && expectedGuest === null) {
    blockers.push(
      `VM name ${vmName} exists but does not own the requested address as one QEMU guest`,
    );
  }
  if (addressMatches.length === 1 && expectedGuest === null) {
    blockers.push(`address ${target} is assigned to a different guest identity`);
  }
  if (vmIdMatches.length > 0 && expectedGuest === null) {
    blockers.push(`proposed VM identifier ${proposedVmId} is already in use`);
  }

  const activeOwner = evidence.address.activeOwnerDetected;
  if (activeOwner && !neighborBelongsToGuest(evidence.address.neighborMac, expectedGuest)) {
    blockers.push(`neighbor discovery found an unrecognized active owner for ${target}`);
  }
  if (evidence.address.probeConclusive !== true) {
    blockers.push(
      `address probe for ${target} was inconclusive (${evidence.address.neighborState})`,
    );
  }

  const result = {
    selectedBridge: bridge ?? null,
    proposedVmId,
    expectedGuest,
    checks: {
      dns: {
        localA: evidence.dns.localA,
        publicA: unique(evidence.dns.publicAnswers.flatMap((answer) => answer.a)),
        publicAaaa: unique(evidence.dns.publicAnswers.flatMap((answer) => answer.aaaa)),
      },
      inventory: {
        vmNameMatches: nameMatches.map((guest) => guest.vmId),
        addressMatches: addressMatches.map((guest) => guest.vmId),
        vmIdAvailable: vmIdMatches.length === 0 || expectedGuest !== null,
      },
      address: {
        routeDevice: evidence.address.routeDevice,
        neighborState: evidence.address.neighborState,
        activeOwnerDetected: activeOwner,
      },
      ambiguities: blockers,
      conflictFree: blockers.length === 0,
    },
  };
  return result;
}

export function hostProbeRunner({ ip = "ip", ping = "ping" } = {}) {
  return {
    async route(targetAddress) {
      const route = await jsonCommand(ip, ["-json", "route", "get", targetAddress]);
      assert(Array.isArray(route) && route.length === 1, `expected one route to ${targetAddress}`);
      assert(typeof route[0].dev === "string" && route[0].dev !== "", "route has no device");
      return { device: route[0].dev, onLink: route[0].gateway === undefined };
    },
    async probeAddress(targetAddress, device) {
      try {
        await execFile(ping, ["-c", "2", "-W", "1", targetAddress], {
          encoding: "utf8",
          timeout: 5_000,
        });
      } catch {
        // A failed ICMP exchange still triggers the required on-link ARP probe.
      }
      const neighbors = await jsonCommand(ip, [
        "-json",
        "neigh",
        "show",
        "to",
        targetAddress,
        "dev",
        device,
      ]);
      const neighbor = neighbors[0];
      const neighborState = normalizeNeighborState(neighbor?.state);
      const neighborMac =
        typeof neighbor?.lladdr === "string" ? neighbor.lladdr.toLowerCase() : null;
      return {
        neighborState,
        neighborMac,
        activeOwnerDetected: neighborMac !== null && activeNeighborStates.has(neighborState),
        probeConclusive: neighborState !== "unknown",
      };
    },
  };
}

export function defaultDnsResolvers() {
  return {
    local: new Resolver(),
    public: [resolver("1.1.1.1"), resolver("8.8.8.8")],
  };
}

async function resolveDnsEvidence(fqdn, resolvers) {
  const errors = [];
  const localA = await resolveFamily(resolvers.local, "resolve4", fqdn, "local", errors);
  const publicAnswers = [];
  for (const publicResolver of resolvers.public) {
    const server = publicResolver.getServers().join(",");
    publicAnswers.push({
      server,
      a: await resolveFamily(publicResolver, "resolve4", fqdn, server, errors),
      aaaa: await resolveFamily(publicResolver, "resolve6", fqdn, server, errors, true),
    });
  }
  return { localA: unique(localA), publicAnswers, errors };
}

async function resolveFamily(resolverInstance, method, fqdn, server, errors, emptyIsValid = false) {
  try {
    return unique(await resolverInstance[method](fqdn));
  } catch (error) {
    if (emptyIsValid && ["ENODATA", "ENOTFOUND"].includes(error?.code)) return [];
    errors.push(`${server} ${method} failed for ${fqdn}: ${safeError(error)}`);
    return [];
  }
}

async function jsonCommand(command, args) {
  try {
    const { stdout } = await execFile(command, args, {
      encoding: "utf8",
      maxBuffer: 1024 * 1024,
      timeout: 10_000,
    });
    return JSON.parse(stdout);
  } catch (error) {
    throw new Error(`read-only host probe failed: ${safeError(error)}`);
  }
}

function resolver(server) {
  const instance = new Resolver();
  instance.setServers([server]);
  return instance;
}

function normalizeNeighborState(value) {
  const raw = Array.isArray(value) ? value[0] : value;
  const normalized = typeof raw === "string" ? raw.toLowerCase() : "absent";
  return [
    "absent",
    "failed",
    "incomplete",
    "reachable",
    "stale",
    "delay",
    "probe",
    "permanent",
  ].includes(normalized)
    ? normalized
    : "unknown";
}

function guestHasAddress(guest, address) {
  return guest.config.ipConfigurations.some(({ value }) =>
    new RegExp(`(?:^|[,=])${escapeRegExp(address)}(?:/|,|$)`).test(value),
  );
}

function neighborBelongsToGuest(neighborMac, guest) {
  if (neighborMac === null || guest === null) return false;
  return guest.config.networks.some(({ value }) => value.toLowerCase().includes(neighborMac));
}

function ipv4SubnetContains(address, prefixLength, target) {
  if (!isIpv4(address) || !isIpv4(target) || !Number.isInteger(prefixLength)) return false;
  const mask = prefixLength === 0 ? 0 : (0xffffffff << (32 - prefixLength)) >>> 0;
  return (ipv4Number(address) & mask) === (ipv4Number(target) & mask);
}

function ipv4Number(value) {
  return value.split(".").reduce((result, octet) => (result << 8) + Number(octet), 0) >>> 0;
}

function isIpv4(value) {
  if (typeof value !== "string") return false;
  const parts = value.split(".").map(Number);
  return (
    parts.length === 4 && parts.every((part) => Number.isInteger(part) && part >= 0 && part <= 255)
  );
}

function sameSet(left, right) {
  return JSON.stringify(unique(left)) === JSON.stringify(unique(right));
}

function unique(values) {
  return [...new Set(values)].sort();
}

function list(values) {
  return values.length === 0 ? "<none>" : values.join(", ");
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function safeError(error) {
  return (error instanceof Error ? error.message : String(error))
    .replace(/[\r\n]+/g, " ")
    .slice(0, 300);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
