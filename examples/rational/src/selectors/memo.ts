/**
 * Remember the last result of a pure function keyed by the identity of its
 * arguments. RxDB query results are new arrays only when the underlying
 * documents change, so this keeps derived numbers stable between renders.
 */
export function memoizeLast<Args extends readonly unknown[], Result>(
  compute: (...args: Args) => Result,
): (...args: Args) => Result {
  let lastArgs: Args | undefined;
  let lastResult: Result | undefined;
  return (...args: Args): Result => {
    if (
      lastArgs !== undefined &&
      lastArgs.length === args.length &&
      lastArgs.every((value, index) => Object.is(value, args[index]))
    ) {
      return lastResult as Result;
    }
    lastArgs = args;
    lastResult = compute(...args);
    return lastResult;
  };
}
