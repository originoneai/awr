/** Format a unix-ms timestamp for freshness/empty-state copy. */
export function formatUnixMs(unixMs: number, locale?: string): string {
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  }).format(new Date(unixMs));
}
