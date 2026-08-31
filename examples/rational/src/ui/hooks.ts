import { useEffect, useRef, useState } from "react";
import type { RxQuery } from "rxdb";
import type { BehaviorSubject, Observable } from "rxjs";

/** Render the latest value of a subject; the subject's current value is the first render. */
export function useBehavior<T>(subject: BehaviorSubject<T>): T {
  const [value, setValue] = useState<T>(() => subject.value);
  useEffect(() => {
    const subscription = subject.subscribe(setValue);
    return () => subscription.unsubscribe();
  }, [subject]);
  return value;
}

export function useObservable<T>(observable: Observable<T> | null, initial: T): T {
  const [value, setValue] = useState<T>(initial);
  useEffect(() => {
    if (observable === null) return undefined;
    const subscription = observable.subscribe(setValue);
    return () => subscription.unsubscribe();
  }, [observable]);
  return value;
}

/**
 * The documents of an RxDB query as plain objects, re-rendering when they
 * change. The query is created by the caller each render; RxDB caches equal
 * queries, so the subscription only moves when the query actually differs.
 */
export function useQuery<T>(query: RxQuery<T, unknown[]> | null): readonly T[] {
  const [documents, setDocuments] = useState<readonly T[]>([]);
  const latest = useRef(query);
  latest.current = query;
  // The key captures the query's identity across renders: same collection,
  // same database generation, same mango query.
  const key =
    query === null
      ? ""
      : `${query.collection.database.name}:${query.collection.name}:${JSON.stringify(query.mangoQuery)}`;
  useEffect(() => {
    const current = latest.current;
    if (current === null || key === "") {
      setDocuments([]);
      return undefined;
    }
    const subscription = current.$.subscribe((results) => {
      setDocuments((results as Array<{ toJSON(): T }>).map((document) => document.toJSON()));
    });
    return () => subscription.unsubscribe();
  }, [key]);
  return documents;
}
