/**
 * The application's view of the shared rules engine.
 *
 * The implementation lives in `functions/shared/rules.ts` because the nightly
 * function uses the same one, and a function bundle can only carry files
 * copied into its directory. This file is where the application's own types
 * meet it, so a caller here works with `Rule` and `Transaction` rather than
 * the structural shapes the shared module declares.
 */
export {
  applyRules,
  countMatches,
  pendingRecategorization,
  ruleMatches,
  ruleStatesSomething,
  sortRules,
  type RuleLike,
  type RuleOutcome,
  type RuleSubject,
} from "../../functions/shared/rules.js";
