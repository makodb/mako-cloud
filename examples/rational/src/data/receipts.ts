import { MakoStorageClient, type MakoStorageObjectRecord } from "@mako-cloud/rxdb";

import type { RationalAuth } from "../auth.js";
import type { RationalConfig } from "../config.js";
import { makoConfigFor } from "./replication.js";
import type { Transport } from "./transport.js";

/** The bucket `mako/buckets/receipts.json` installs. */
export const RECEIPTS_BUCKET = "receipts";
/** What a receipt may be, matching the bucket's own limits. */
export const RECEIPT_CONTENT_TYPES = ["image/png", "image/jpeg", "image/webp", "application/pdf"];
export const MAX_RECEIPT_BYTES = 8 * 1024 * 1024;

export interface Receipt {
  /** The object path, which is what the transaction stores. */
  readonly path: string;
  readonly name: string;
  readonly contentType: string;
  readonly sizeBytes: number;
}

/**
 * Receipts for one household.
 *
 * A receipt belongs to the household, not to whoever photographed it, so the
 * object carries `household_id` as an attribute and the bucket's rules read
 * it: `claims.households[old.attributes.household_id] != null`. The attribute
 * names the household and the **claim** decides, so attaching one the caller
 * is not a member of matches no claim and grants nothing -- the same shape as
 * a document carrying a `household_id` the policy checks.
 *
 * Before objects could carry attributes the rule could only reach the
 * uploader, and a receipt was invisible to everybody else in the household
 * (findings log #8).
 */
export class Receipts {
  readonly #client: MakoStorageClient;
  readonly #householdId: string;

  constructor(
    config: RationalConfig,
    auth: RationalAuth,
    transport: Transport,
    householdId: string,
  ) {
    // The scope is the normalized one the replication uses, so the ids are
    // parsed exactly once and in one place.
    const scope = makoConfigFor(config, "transactions", householdId);
    this.#client = new MakoStorageClient(
      {
        endpoint: scope.endpoint,
        projectId: scope.projectId,
        environmentId: scope.environmentId,
        publicProjectKey: scope.publicProjectKey,
      },
      auth.client,
      { fetch: transport.fetch },
    );
    this.#householdId = householdId;
  }

  /** Where a transaction's receipts live. */
  prefix(transactionId: string): string {
    return `households/${this.#householdId}/transactions/${transactionId}/`;
  }

  async attach(transactionId: string, file: File): Promise<Receipt> {
    if (!RECEIPT_CONTENT_TYPES.includes(file.type)) {
      throw new Error(`${file.type || "that file"} is not an image or a PDF`);
    }
    if (file.size > MAX_RECEIPT_BYTES) {
      throw new Error("a receipt may be at most 8 MB");
    }
    const path = `${this.prefix(transactionId)}${safeName(file.name)}`;
    await this.#client.put(RECEIPTS_BUCKET, path, file, {
      contentType: file.type,
      // What the bucket's rules read. Every member of this household may open
      // it; nobody else may, whatever they attach to their own uploads.
      attributes: { household_id: this.#householdId },
    });
    return { path, name: file.name, contentType: file.type, sizeBytes: file.size };
  }

  /** The stored receipts under a transaction, whoever in the household added them. */
  async list(transactionId: string): Promise<readonly Receipt[]> {
    const page = await this.#client.list(RECEIPTS_BUCKET, {
      prefix: this.prefix(transactionId),
      limit: 50,
    });
    return page.items.map(describe);
  }

  /** The bytes, for showing the receipt; null when it is no longer stored. */
  async open(path: string): Promise<Blob | null> {
    const object = await this.#client.get(RECEIPTS_BUCKET, path);
    if (object === null) return null;
    return new Blob([object.bytes as BlobPart], { type: object.contentType });
  }

  async remove(path: string): Promise<void> {
    await this.#client.delete(RECEIPTS_BUCKET, path);
  }

  /** Every receipt of a transaction, for deleting the transaction itself. */
  async removeAll(transactionId: string): Promise<void> {
    for (const receipt of await this.list(transactionId)) {
      await this.remove(receipt.path);
    }
  }
}

function describe(record: MakoStorageObjectRecord): Receipt {
  const name = record.path.slice(record.path.lastIndexOf("/") + 1);
  return {
    path: record.path,
    name,
    contentType: record.contentType,
    sizeBytes: record.sizeBytes,
  };
}

/**
 * A file name an object path may hold: no separators, no control characters,
 * and short enough that the whole path stays inside the platform's 512 bytes.
 * A name that reduces to nothing gets a stable one rather than an empty
 * segment, which the platform would refuse.
 */
export function safeName(name: string): string {
  const cleaned = [...name]
    .map((character) => {
      if (character === "/" || character === "\\") return "-";
      const code = character.codePointAt(0) ?? 0;
      return code < 0x20 || code === 0x7f ? "" : character;
    })
    .join("")
    .trim()
    .slice(0, 96);
  return cleaned === "" || cleaned === "." || cleaned === ".." ? "receipt" : cleaned;
}
