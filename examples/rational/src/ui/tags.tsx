import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  EmptyState,
  Field,
  Input,
} from "@mako-cloud/ui";
import { CircleAlert, Plus, Tag } from "lucide-react";
import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId, TaxonomyEntry } from "../model/types.js";
import { selectTagUsage } from "../selectors/tags.js";
import { useQuery } from "./hooks.js";
import { transactionsHash } from "./router.js";

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
    <section aria-labelledby="tags-title" data-testid="tags-screen" className="grid gap-6">
      <div className="grid gap-1">
        <h1 id="tags-title" className="text-2xl">
          Tags
        </h1>
        <p className="text-sm text-muted-foreground">
          Your own words for what a transaction is, beside its category; each count leads to the
          transactions that carry it.
        </p>
      </div>
      <Card>
        <CardContent>
          <form
            className="flex flex-wrap items-end gap-3"
            onSubmit={(event) => void add(event)}
            aria-label="New tag"
          >
            <Field label="Name" htmlFor="tag-name" className="w-72">
              <Input
                id="tag-name"
                name="name"
                required
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </Field>
            <Button type="submit">
              <Plus />
              Add tag
            </Button>
          </form>
        </CardContent>
      </Card>
      {error === null ? null : (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription className="block">{error}</AlertDescription>
        </Alert>
      )}
      {tags.length === 0 ? (
        <div data-testid="tags-empty">
          <EmptyState
            icon={<Tag />}
            title="No tags yet."
            description="A tag is a word of your own on a transaction, beside its category."
          />
        </div>
      ) : (
        <ul
          className="m-0 list-none divide-y rounded-xl border bg-card p-0 text-sm shadow-xs"
          aria-label="Tags"
        >
          {tags.map((tag) => {
            const count = usage.get(tag.id) ?? 0;
            return (
              <li
                key={tag.id}
                data-testid={`tag-${tag.id}`}
                className="flex flex-wrap items-center gap-3 px-4 py-2.5"
              >
                {renaming?.id === tag.id ? (
                  <Input
                    aria-label={`Rename ${tag.name}`}
                    className="h-8 w-56"
                    value={renaming.name}
                    onChange={(event) => setRenaming({ id: tag.id, name: event.target.value })}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") void rename(tag);
                      if (event.key === "Escape") setRenaming(null);
                    }}
                  />
                ) : (
                  <Badge variant="secondary" className="text-sm">
                    <Tag aria-hidden="true" />
                    {tag.name}
                  </Badge>
                )}
                <a
                  className="text-muted-foreground tabular-nums"
                  data-testid="tag-count"
                  href={transactionsHash({ tag: tag.id })}
                  aria-label={`${count} ${count === 1 ? "transaction" : "transactions"} tagged ${tag.name}`}
                >
                  {count} {count === 1 ? "transaction" : "transactions"}
                </a>
                <span className="ml-auto flex items-center gap-1">
                  {renaming?.id === tag.id ? (
                    <Button variant="ghost" size="sm" onClick={() => void rename(tag)}>
                      Save
                    </Button>
                  ) : (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setRenaming({ id: tag.id, name: tag.name })}
                    >
                      Rename
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-destructive hover:text-destructive"
                    onClick={() => void app.writes?.deleteTag(tag.id)}
                  >
                    Delete
                  </Button>
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
