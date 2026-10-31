// jest-axe ships no TypeScript declarations; axe-core (its dependency) does.
declare module 'jest-axe' {
  import type { AxeResults, RunOptions } from 'axe-core';

  export function axe(
    container: Element | string,
    options?: RunOptions,
  ): Promise<AxeResults>;
  export function configureAxe(options?: RunOptions): typeof axe;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  export const toHaveNoViolations: Record<string, (...args: any[]) => any>;
}
