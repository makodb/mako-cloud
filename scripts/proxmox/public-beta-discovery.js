import { execFile as execFileCallback } from "node:child_process";
import { hostname } from "node:os";
import { promisify } from "node:util";

import { assertNoSecrets } from "./public-beta-plan-lib.js";

const execFile = promisify(execFileCallback);
const allowedPveshRoots = new Set(["/cluster/status", "/cluster/nextid", "/nodes", "/storage"]);

export async function discoverProxmox({ runner = commandRunner(), localNode = hostname() } = {}) {
  const [clusterStatus, nodes, storageConfiguration, nextVmId] = await Promise.all([
    runner.pvesh("/cluster/status"),
    runner.pvesh("/nodes"),
    runner.pvesh("/storage"),
    runner.pvesh("/cluster/nextid"),
  ]);
  const selectedNode = selectNode(nodes, localNode);
  const nodeName = selectedNode.node;
  const [network, dns, storageStatus, qemuGuests, lxcGuests, nodeStatus] = await Promise.all([
    runner.pvesh(`/nodes/${nodeName}/network`),
    runner.pvesh(`/nodes/${nodeName}/dns`),
    runner.pvesh(`/nodes/${nodeName}/storage`),
    runner.pvesh(`/nodes/${nodeName}/qemu`),
    runner.pvesh(`/nodes/${nodeName}/lxc`),
    runner.pvesh(`/nodes/${nodeName}/status`),
  ]);

  const qemu = await Promise.all(
    qemuGuests.map(async (guest) => ({
      type: "qemu",
      vmId: asInteger(guest.vmid, "QEMU VM identifier"),
      name: guest.name ?? "",
      status: normalizeGuestStatus(guest.status),
      config: normalizeQemuConfig(
        await runner.pvesh(`/nodes/${nodeName}/qemu/${guest.vmid}/config`),
      ),
    })),
  );
  const lxc = await Promise.all(
    lxcGuests.map(async (guest) => ({
      type: "lxc",
      vmId: asInteger(guest.vmid, "LXC VM identifier"),
      name: guest.name ?? guest.hostname ?? "",
      status: normalizeGuestStatus(guest.status),
      config: normalizeLxcConfig(await runner.pvesh(`/nodes/${nodeName}/lxc/${guest.vmid}/config`)),
    })),
  );

  const storage = normalizeStorage(storageConfiguration, storageStatus);
  const imageStorage = storage.filter((entry) =>
    entry.content.some((content) => ["images", "iso", "vztmpl"].includes(content)),
  );
  const imageContent = (
    await Promise.all(
      imageStorage.map(async (entry) => {
        try {
          return await runner.pvesh(`/nodes/${nodeName}/storage/${entry.id}/content`);
        } catch (error) {
          return [{ discoveryError: safeError(error), storage: entry.id }];
        }
      }),
    )
  ).flat();

  const result = {
    cluster: clusterName(clusterStatus),
    localNode,
    selectedNode: {
      name: nodeName,
      status: selectedNode.status ?? "unknown",
      cpuCount: asInteger(
        nodeStatus.cpuinfo?.cpus ?? nodeStatus.maxcpu ?? selectedNode.maxcpu,
        "node CPU count",
      ),
      memoryBytes: asInteger(
        nodeStatus.memory?.total ?? nodeStatus.maxmem ?? selectedNode.maxmem,
        "node memory",
      ),
      memoryUsedBytes: asInteger(
        nodeStatus.memory?.used ?? nodeStatus.mem ?? selectedNode.mem ?? 0,
        "node used memory",
      ),
    },
    nextVmId: asInteger(nextVmId, "next VM identifier"),
    nodes: nodes.map((node) => ({
      name: node.node,
      status: node.status ?? "unknown",
      cpuCount: asInteger(node.maxcpu ?? 0, "node CPU count"),
      memoryBytes: asInteger(node.maxmem ?? 0, "node memory"),
      memoryUsedBytes: asInteger(node.mem ?? 0, "node used memory"),
    })),
    networks: normalizeNetworks(network),
    dns: normalizeDns(dns),
    storage,
    guests: [...qemu, ...lxc].sort((left, right) => left.vmId - right.vmId),
    approvedImageCandidates: normalizeImages(imageContent),
  };
  assertNoSecrets(result, "discovery");
  return result;
}

export function commandRunner({
  pvesh = process.env.MAKO_PVESH ?? "pvesh",
  sshTarget = process.env.MAKO_PROXMOX_SSH,
} = {}) {
  return {
    async pvesh(endpoint) {
      assertSafeEndpoint(endpoint);
      try {
        const command = sshTarget ? "ssh" : pvesh;
        const args = sshTarget
          ? [
              "-o",
              "BatchMode=yes",
              "-o",
              "ConnectTimeout=10",
              sshTarget,
              pvesh,
              "get",
              endpoint,
              "--output-format",
              "json",
            ]
          : ["get", endpoint, "--output-format", "json"];
        const { stdout } = await execFile(command, args, {
          encoding: "utf8",
          maxBuffer: 8 * 1024 * 1024,
          timeout: 30_000,
        });
        return JSON.parse(stdout);
      } catch (error) {
        const message = safeError(error);
        throw new Error(
          `read-only Proxmox discovery failed for ${endpoint}: ${message}. ` +
            "Run as a Proxmox identity with PVEAuditor-equivalent read access; do not grant mutation solely for planning.",
        );
      }
    },
  };
}

export function selectNode(nodes, localNode) {
  assert(Array.isArray(nodes) && nodes.length > 0, "Proxmox returned no nodes");
  const local = nodes.find((node) => node.node === localNode);
  if (local !== undefined && local.status !== "offline") return local;
  const online = nodes.filter((node) => node.status === "online");
  assert(
    online.length === 1,
    `cannot derive one target node: local node ${localNode} is unavailable and ${online.length} nodes are online`,
  );
  return online[0];
}

function assertSafeEndpoint(endpoint) {
  const dynamic =
    /^\/nodes\/[a-zA-Z0-9_.-]+\/(?:network|dns|storage|status|qemu|lxc)(?:\/[a-zA-Z0-9_.:-]+(?:\/config|\/content)?)?$/;
  assert(
    allowedPveshRoots.has(endpoint) || dynamic.test(endpoint),
    `unsafe Proxmox endpoint: ${endpoint}`,
  );
}

function normalizeNetworks(network) {
  assert(Array.isArray(network), "node network inventory must be an array");
  return network
    .filter((entry) => entry.type === "bridge" && truthy(entry.active))
    .map((entry) => ({
      bridge: entry.iface,
      address: entry.address ?? null,
      prefixLength: prefixLength(entry.cidr, entry.netmask),
      gateway: entry.gateway ?? null,
      bridgePorts: String(entry.bridge_ports ?? "")
        .split(/\s+/)
        .filter(Boolean),
    }))
    .sort((left, right) => left.bridge.localeCompare(right.bridge));
}

function normalizeDns(dns) {
  assert(
    dns !== null && typeof dns === "object" && !Array.isArray(dns),
    "node DNS inventory must be an object",
  );
  return {
    servers: [dns.dns1, dns.dns2, dns.dns3]
      .filter((value) => typeof value === "string" && value !== "")
      .filter((value, index, values) => values.indexOf(value) === index),
    search: typeof dns.search === "string" ? dns.search : "",
  };
}

function normalizeStorage(configuration, status) {
  assert(Array.isArray(configuration) && Array.isArray(status), "storage inventory must be arrays");
  const configs = new Map(configuration.map((entry) => [entry.storage, entry]));
  return status
    .filter((entry) => truthy(entry.active) && truthy(entry.enabled ?? 1))
    .map((entry) => {
      const config = configs.get(entry.storage) ?? {};
      return {
        id: entry.storage,
        type: entry.type ?? config.type ?? "unknown",
        content: splitCsv(entry.content ?? config.content),
        availableBytes: asInteger(entry.avail ?? 0, "storage available bytes"),
        totalBytes: asInteger(entry.total ?? 0, "storage total bytes"),
        shared: truthy(entry.shared ?? config.shared ?? 0),
        nodes: splitCsv(config.nodes),
      };
    })
    .sort((left, right) => left.id.localeCompare(right.id));
}

function normalizeImages(content) {
  return content
    .filter(
      (entry) =>
        entry.discoveryError !== undefined || /ubuntu|noble|24[.-]?04/i.test(entry.volid ?? ""),
    )
    .map((entry) =>
      entry.discoveryError === undefined
        ? {
            storage: String(entry.volid).split(":", 1)[0],
            volume: entry.volid,
            format: entry.format ?? "unknown",
            sizeBytes: asInteger(entry.size ?? 0, "image size"),
          }
        : { storage: entry.storage, discoveryError: entry.discoveryError },
    );
}

function normalizeQemuConfig(config) {
  return {
    digest: config.digest ?? null,
    name: config.name ?? "",
    description: config.description ?? "",
    hardware: {
      bios: config.bios ?? null,
      machine: config.machine ?? null,
      cpu: config.cpu ?? null,
      cores: numberOrNull(config.cores),
      sockets: numberOrNull(config.sockets),
      memoryMiB: numberOrNull(config.memory),
      agent: config.agent ?? null,
      boot: config.boot ?? null,
      disks: Object.entries(config)
        .filter(([key]) => /^(?:scsi|sata|virtio|ide)\d+$/.test(key))
        .map(([key, value]) => ({ key, value: String(value) })),
    },
    ipConfigurations: Object.entries(config)
      .filter(([key]) => /^ipconfig\d+$/.test(key))
      .map(([key, value]) => ({ key, value: String(value) })),
    networks: Object.entries(config)
      .filter(([key]) => /^net\d+$/.test(key))
      .map(([key, value]) => ({ key, value: String(value) })),
  };
}

function normalizeLxcConfig(config) {
  return {
    digest: config.digest ?? null,
    name: config.hostname ?? "",
    ipConfigurations: Object.entries(config)
      .filter(([key]) => /^net\d+$/.test(key))
      .map(([key, value]) => ({ key, value: String(value) })),
    networks: [],
  };
}

function normalizeGuestStatus(value) {
  return ["running", "stopped"].includes(value) ? value : "unknown";
}

function clusterName(status) {
  assert(Array.isArray(status), "cluster status must be an array");
  const cluster = status.find((entry) => entry.type === "cluster");
  return cluster?.name ?? "standalone";
}

function prefixLength(cidr, netmask) {
  if (typeof cidr === "string" && cidr.includes("/")) return Number(cidr.split("/").at(-1));
  if (typeof netmask !== "string") return null;
  return netmask
    .split(".")
    .map(Number)
    .reduce((bits, octet) => bits + octet.toString(2).replaceAll("0", "").length, 0);
}

function splitCsv(value) {
  if (typeof value !== "string" || value === "") return [];
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean)
    .sort();
}

function truthy(value) {
  return value === true || value === 1 || value === "1";
}

function numberOrNull(value) {
  if (value === undefined || value === null || value === "") return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function asInteger(value, name) {
  const parsed = Number(value);
  assert(Number.isSafeInteger(parsed) && parsed >= 0, `${name} is invalid`);
  return parsed;
}

function safeError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/[\r\n]+/g, " ").slice(0, 500);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
