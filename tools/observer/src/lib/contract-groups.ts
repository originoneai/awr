import type { WorkContractValue } from './types';

export interface ContractSourceWork {
  title: string;
  goals?: WorkContractValue[];
  acceptance?: WorkContractValue[];
}

export interface ContractGroup {
  titles: string[];
  goals?: WorkContractValue[];
  acceptance?: WorkContractValue[];
}

/** Works that agreed on their goals and acceptance are shown once, under all their names. */
export function contractGroups(works: ReadonlyArray<ContractSourceWork>): ContractGroup[] {
  const groups = new Map<string, ContractGroup>();
  for (const work of works) {
    if (!work.goals?.length && !work.acceptance?.length) continue;
    const key = JSON.stringify([work.goals ?? null, work.acceptance ?? null]);
    const group = groups.get(key);
    if (group) group.titles.push(work.title);
    else groups.set(key, { titles: [work.title], goals: work.goals, acceptance: work.acceptance });
  }
  return Array.from(groups.values());
}
