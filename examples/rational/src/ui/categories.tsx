import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { CategoryKind, HouseholdCollectionId, TaxonomyEntry } from "../model/types.js";
import { useQuery } from "./hooks.js";

const KINDS: readonly CategoryKind[] = ["expense", "income", "transfer"];

export function CategoriesScreen({
  app,
  session,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
}) {
  const categories = useQuery(
    session
      .collection("taxonomy")
      ?.find({ selector: { kind: "category" }, sort: [{ name: "asc" }] }) ?? null,
  );
  const [name, setName] = useState("");
  const [kind, setKind] = useState<CategoryKind>("expense");
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);

  const add = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    try {
      await app.writes?.createCategory(name, kind);
      setName("");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "The category could not be saved.");
    }
  };

  const rename = async (category: TaxonomyEntry) => {
    if (renaming === null) return;
    try {
      await app.writes?.updateCategory(category.id, { name: renaming.name });
      setRenaming(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "The category could not be renamed.");
    }
  };

  return (
    <section aria-labelledby="categories-title">
      <div className="heading">
        <h1 id="categories-title">Categories</h1>
      </div>
      <form className="inline" onSubmit={(event) => void add(event)} aria-label="New category">
        <label>
          Name
          <input
            name="name"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          Kind
          <select
            name="kind"
            value={kind}
            onChange={(event) => setKind(event.target.value as CategoryKind)}
          >
            {KINDS.map((candidate) => (
              <option key={candidate} value={candidate}>
                {candidate}
              </option>
            ))}
          </select>
        </label>
        <button type="submit">Add category</button>
      </form>
      {error === null ? null : (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <table className="list" aria-label="Categories">
        <thead>
          <tr>
            <th scope="col">Name</th>
            <th scope="col">Kind</th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {categories.map((category) => (
            <tr
              key={category.id}
              data-testid={`category-${category.id}`}
              className={category.archived === true ? "muted" : undefined}
            >
              <td>
                {renaming?.id === category.id ? (
                  <input
                    aria-label={`Rename ${category.name}`}
                    value={renaming.name}
                    onChange={(event) => setRenaming({ id: category.id, name: event.target.value })}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") void rename(category);
                      if (event.key === "Escape") setRenaming(null);
                    }}
                  />
                ) : (
                  <span>
                    {category.name}
                    {category.archived === true ? <small> archived</small> : null}
                  </span>
                )}
              </td>
              <td>{category.category_kind}</td>
              <td className="actions">
                {renaming?.id === category.id ? (
                  <button type="button" className="link" onClick={() => void rename(category)}>
                    Save
                  </button>
                ) : (
                  <button
                    type="button"
                    className="link"
                    onClick={() => setRenaming({ id: category.id, name: category.name })}
                  >
                    Rename
                  </button>
                )}
                <button
                  type="button"
                  className="link"
                  onClick={() =>
                    void app.writes?.updateCategory(category.id, {
                      archived: category.archived === true ? null : true,
                    })
                  }
                >
                  {category.archived === true ? "Restore" : "Archive"}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
