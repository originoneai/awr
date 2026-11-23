import { renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import i18n from '../i18n';
import {
  DisplayTimeZoneProvider,
  readClockOffsetUnixMs,
  readMomentUnixMs,
  referenceNowUnixMs,
  useDisplayTimeZone,
  validDisplayTimeZone,
  type ClockedReply,
} from './display-clock';

function reply(overrides: {
  nowUnixMs?: number | null;
  readAtUnixMs?: number | null;
} = {}): ClockedReply {
  const nowUnixMs = overrides.nowUnixMs ?? null;
  const readAt = overrides.readAtUnixMs ?? null;
  return {
    read_at_unix_ms: (readAt ?? Number.NaN) as number,
    input: { options: { nowUnixMs: (nowUnixMs ?? Number.NaN) as number } },
  };
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('readClockOffsetUnixMs', () => {
  it('returns the derivation clock relative to the reply\'s own read time', () => {
    expect(readClockOffsetUnixMs(reply({ nowUnixMs: 1_000, readAtUnixMs: 400 }))).toBe(600);
    expect(readClockOffsetUnixMs(reply({ nowUnixMs: 400, readAtUnixMs: 1_000 }))).toBe(-600);
  });

  it('returns null when the reply carries no usable clock pair', () => {
    expect(readClockOffsetUnixMs(null)).toBeNull();
    expect(readClockOffsetUnixMs(undefined)).toBeNull();
    expect(readClockOffsetUnixMs(reply({ nowUnixMs: null, readAtUnixMs: 1_000 }))).toBeNull();
    expect(readClockOffsetUnixMs(reply({ nowUnixMs: 1_000, readAtUnixMs: null }))).toBeNull();
    expect(readClockOffsetUnixMs(reply({ nowUnixMs: Number.NaN, readAtUnixMs: 1_000 }))).toBeNull();
  });
});

describe('readMomentUnixMs', () => {
  it('is the derivation clock itself: the read time plus the reply\'s offset', () => {
    expect(readMomentUnixMs(reply({ nowUnixMs: 1_000, readAtUnixMs: 400 }))).toBe(1_000);
    expect(readMomentUnixMs(reply({ nowUnixMs: 400, readAtUnixMs: 1_000 }))).toBe(400);
  });

  it('keeps the read time when the reply carries no derivation clock', () => {
    expect(readMomentUnixMs(reply({ nowUnixMs: null, readAtUnixMs: 1_000 }))).toBe(1_000);
    expect(readMomentUnixMs(reply({ nowUnixMs: Number.NaN, readAtUnixMs: 1_000 }))).toBe(1_000);
  });

  it('is unknown without a usable read time', () => {
    expect(readMomentUnixMs(null)).toBeUndefined();
    expect(readMomentUnixMs(undefined)).toBeUndefined();
    expect(readMomentUnixMs(reply({ nowUnixMs: 1_000, readAtUnixMs: null }))).toBeUndefined();
  });

  it('puts a displayed age at the real time since the read, whatever clock the reply derives against', () => {
    // Real read at 10_000, fixture clock far behind at 3_000: the reference walks forward from the
    // fixture clock, and the age counted from the read moment is the 250 real ms since the read.
    const answered = reply({ nowUnixMs: 3_000, readAtUnixMs: 10_000 });
    vi.useFakeTimers({ toFake: ['Date'] });
    try {
      vi.setSystemTime(10_250);
      const reference = referenceNowUnixMs(readClockOffsetUnixMs(answered));
      expect(reference - (readMomentUnixMs(answered) ?? Number.NaN)).toBe(250);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('referenceNowUnixMs', () => {
  it('walks the reply clock forward with the viewer clock', () => {
    vi.useFakeTimers();
    vi.setSystemTime(50_000);
    try {
      expect(referenceNowUnixMs(1_200)).toBe(51_200);
    } finally {
      vi.useRealTimers();
    }
  });

  it('falls back to the viewer clock exactly when no reply clock exists', () => {
    vi.useFakeTimers();
    vi.setSystemTime(50_000);
    try {
      expect(referenceNowUnixMs(null)).toBe(50_000);
      expect(referenceNowUnixMs(undefined)).toBe(50_000);
      expect(referenceNowUnixMs(Number.NaN)).toBe(50_000);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('validDisplayTimeZone', () => {
  it('accepts zones the runtime resolves and rejects everything else', () => {
    expect(validDisplayTimeZone('UTC')).toBe('UTC');
    expect(validDisplayTimeZone('Asia/Shanghai')).toBe('Asia/Shanghai');
    expect(validDisplayTimeZone('Not/AZone')).toBeNull();
    expect(validDisplayTimeZone('')).toBeNull();
    expect(validDisplayTimeZone(7)).toBeNull();
    expect(validDisplayTimeZone(null)).toBeNull();
  });
});

describe('useDisplayTimeZone', () => {
  it('returns the provider\'s pinned zone inside the provider', () => {
    const viewerZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const inside = renderHook(() => useDisplayTimeZone(), { wrapper: ({ children }) => (
      <DisplayTimeZoneProvider timeZone="UTC">{children}</DisplayTimeZoneProvider>
    ) });
    expect(inside.result.current).toBe('UTC');
    // Without a pinned zone the viewer's own zone stays the display zone.
    const outside = renderHook(() => useDisplayTimeZone());
    expect(outside.result.current).toBe(viewerZone);
  });

  it('ignores a pinned zone the runtime cannot resolve', () => {
    const viewerZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const { result } = renderHook(() => useDisplayTimeZone(), { wrapper: ({ children }) => (
      <DisplayTimeZoneProvider timeZone="Not/AZone">{children}</DisplayTimeZoneProvider>
    ) });
    expect(result.current).toBe(viewerZone);
  });
});
