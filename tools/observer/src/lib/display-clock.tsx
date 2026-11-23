import { createContext, useContext, useMemo, type ReactNode } from 'react';
import type { TeamOverviewModelResponse } from './types';

/** A reply that carries a derivation clock; the derivation fields stay optional so an
 * older or partial reply reads as "no clock" instead of throwing. */
export type ClockedReply = Pick<TeamOverviewModelResponse, 'read_at_unix_ms'> & {
  input?: { options?: { nowUnixMs?: number } };
};

/**
 * The reply's own derivation clock relative to the moment it was read:
 * `input.options.nowUnixMs - read_at_unix_ms`. Adding this to the viewer's clock walks
 * the reply's "now" forward in step with real time, which is what displayed ages need:
 * the read's fixed clock would freeze "12 分钟前" forever, and the reply's read time is
 * the answer moment, never the organization's "now". Null when the reply carries no
 * usable pair — the caller then falls back to the viewer's own clock.
 */
export function readClockOffsetUnixMs(model: ClockedReply | null | undefined): number | null {
  const nowUnixMs = model?.input?.options?.nowUnixMs;
  const readAtUnixMs = model?.read_at_unix_ms;
  return typeof nowUnixMs === 'number' && Number.isFinite(nowUnixMs)
    && typeof readAtUnixMs === 'number' && Number.isFinite(readAtUnixMs)
    ? nowUnixMs - readAtUnixMs
    : null;
}

/**
 * The reference clock every page formats times against: the reply's derivation clock
 * walked forward (`Date.now() + offset`), or the viewer's own clock when no reply clock
 * exists. This is the only read of the clock the pages go through; pages themselves never
 * call Date.now() (a guard test scans them).
 */
export function referenceNowUnixMs(offsetUnixMs: number | null | undefined): number {
  return typeof offsetUnixMs === 'number' && Number.isFinite(offsetUnixMs)
    ? Date.now() + offsetUnixMs
    : Date.now();
}

/** A time zone the runtime actually resolves, or null: the fact is only displayed when
 * the viewer's environment can honor it, so a bogus value reads as "not pinned". */
export function validDisplayTimeZone(zone: unknown): string | null {
  if (typeof zone !== 'string' || zone.length === 0) return null;
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: zone });
    return zone;
  } catch {
    return null;
  }
}

/** The zone this deployment pins for display (canonical demo: UTC). Null keeps the
 * viewer's own zone, which live reads and every other variant use. */
const DisplayTimeZoneContext = createContext<string | null>(null);

export function DisplayTimeZoneProvider({ timeZone, children }: {
  timeZone: string | null;
  children: ReactNode;
}) {
  const value = useMemo(() => validDisplayTimeZone(timeZone), [timeZone]);
  return <DisplayTimeZoneContext.Provider value={value}>{children}</DisplayTimeZoneContext.Provider>;
}

/** The zone pages format displayed times in: the deployment's pinned zone when the
 * shell read established one, else the viewer's own zone. */
export function useDisplayTimeZone(): string {
  const pinned = validDisplayTimeZone(useContext(DisplayTimeZoneContext));
  return pinned ?? Intl.DateTimeFormat().resolvedOptions().timeZone;
}
