import { describe, expect, it } from 'vitest';
import { queryKeys, sourceContentQueryOptions } from './queries';

/**
 * The cadence of the full-text read, pinned where it is declared. A read the person asked for is
 * sent once: it is not polled, not retried, not repeated when the window regains focus, when the
 * network reconnects or when the page mounts again, and its result is kept for the session. A
 * fake clock cannot exercise a polling interval of the query client under jsdom, so the options
 * themselves are the contract.
 */
describe('sourceContentQueryOptions: a read asked for once is sent once', () => {
  const SHA = 'c101fd5c70986de856c0bd30d2da271b71074408acbec05fabc9fa095e1b80ac';
  const options = sourceContentQueryOptions('demo', 'docs/support.md', SHA);

  it('never polls, retries or re-reads on its own', () => {
    expect(options.refetchInterval).toBe(false);
    expect(options.retry).toBe(false);
    expect(options.refetchOnWindowFocus).toBe(false);
    expect(options.refetchOnReconnect).toBe(false);
    expect(options.refetchOnMount).toBe(false);
  });

  it('keeps the result for the session', () => {
    expect(options.staleTime).toBe(Infinity);
    expect(options.gcTime).toBe(Infinity);
  });

  it('is keyed by project, path and digest, so two versions of one path never share a result', () => {
    const other = sourceContentQueryOptions('demo', 'docs/support.md', 'f'.repeat(64));
    expect(options.queryKey).toEqual(queryKeys.sourceContent('demo', 'docs/support.md', SHA));
    expect(options.queryKey).not.toEqual(other.queryKey);
    expect(options.queryKey).not.toEqual(sourceContentQueryOptions('demo', 'docs/other.md', SHA).queryKey);
    expect(options.queryKey).not.toEqual(sourceContentQueryOptions('other', 'docs/support.md', SHA).queryKey);
  });

  it('is silent once access is revoked', () => {
    expect(options.enabled).toBe(true);
    expect(sourceContentQueryOptions('demo', 'docs/support.md', SHA, true).enabled).toBe(false);
  });
});
