import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId, TaxonomyEntry } from "../model/types.js";
import { selectTagUsage } from "../selectors/tags.js";
import { useQuery } from "./hooks.js";
import { transactionsHash } from "./router.js";
import "./styles/settings-pages.css";

/**
 * Tags: the household's own words for what a transaction is besides its
 * category -- shared, reimbursable, a trip. Each shows how many transactions
 * carry it, and the count is a link to exactly those transactions, so a tag
 * nobody uses is easy to spot before it is deleted.
 */
export function TagsScreen({
  app,
  session,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
}) {
  const tags = useQuery(
    session.collection("taxonomy")?.find({ selector: { kind: "tag" }, sort: [{ name: "asc" }] }) ??
      null,
  );
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const usage = selectTagUsage(transactions);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);

  const add = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    try {
      await app.writes?.createTag(name);
      setName("");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "The tag could not be saved.");
    }
  };

  const rename = async (tag: TaxonomyEntry) => {
    if (renaming === null) return;
    try {
      await app.writes?.updateTag(tag.id, { name: renaming.name });
      setRenaming(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "The tag could not be renamed.");
    }
  };

  return (
    <section aria-labelledby="tags-title" data-testid="tags-screen">
      <div className="heading">
        <h1 id="tags-title">Tags</h1>
      </div>
      <form className="inline" onSubmit={(event) => void add(event)} aria-label="New tag">
        <label>
          Name
          <input
            name="name"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <button type="submit">Add tag</button>
      </form>
      {error === null ? null : (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {tags.length === 0 ? (
        <p className="muted" data-testid="tags-empty">
          No tags yet. A tag is a word of your own on a transaction, beside its category.
        </p>
      ) : (
        <ul className="chips" aria-label="Tags">
          {tags.map((tag) => {
            const count = usage.get(tag.id) ?? 0;
            return (
              <li key={tag.id} data-testid={`tag-${tag.id}`}>
                {renaming?.id === tag.id ? (
                  <input
                    aria-label={`Rename ${tag.name}`}
                    value={renaming.name}
                    onChange={(event) => setRenaming({ id: tag.id, name: event.target.value })}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") void rename(tag);
                      if (event.key === "Escape") setRenaming(null);
                    }}
                  />
                ) : (
                  <span className="chip">{tag.name}</span>
                )}
                <a
                  className="tag-count"
                  data-testid="tag-count"
                  href={transactionsHash({ tag: tag.id })}
                  aria-label={`${count} ${count === 1 ? "transaction" : "transactions"} tagged ${tag.name}`}
                >
                  {count} {count === 1 ? "transaction" : "transactions"}
                </a>
                {renaming?.id === tag.id ? (
                  <button type="button" className="link" onClick={() => void rename(tag)}>
                    Save
                  </button>
                ) : (
                  <button
                    type="button"
                    className="link"
                    onClick={() => setRenaming({ id: tag.id, name: tag.name })}
                  >
                    Rename
                  </button>
                )}
                <button
                  type="button"
                  className="link"
                  onClick={() => void app.writes?.deleteTag(tag.id)}
                >
                  Delete
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
