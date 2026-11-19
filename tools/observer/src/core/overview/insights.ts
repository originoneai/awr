import { deriveChains } from './chains';
import { derivePeople } from './people';
import { buildPendingIndex } from './shared';
import type { OverviewInput, OverviewStatusWaitingDerivation } from './types';
import { deriveViews } from './views';

/** Add the second slice to the completed first slice; no first-slice derivation is repeated here. */
export function deriveOverviewInsights(input: OverviewInput, base: OverviewStatusWaitingDerivation) {
  const pending = buildPendingIndex(input, base);
  const chainResult = deriveChains(input, base);
  const peopleResult = derivePeople(input, base, pending);
  const viewResult = deriveViews(input, base, chainResult, pending);
  return {
    chains: chainResult.chains,
    chainCounts: chainResult.chainCounts,
    people: peopleResult.people,
    peopleCounts: peopleResult.peopleCounts,
    views: viewResult.views,
    coverage: {
      ...base.coverage,
      sections: {
        ...base.coverage.sections,
        chains: { complete: chainResult.coverageIssues.length === 0, issues: chainResult.coverageIssues },
        people: { complete: peopleResult.coverageIssues.length === 0, issues: peopleResult.coverageIssues },
        views: { complete: viewResult.coverageIssues.length === 0, issues: viewResult.coverageIssues },
      },
    },
  };
}
