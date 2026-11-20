import { createLucideIcon, type LucideIcon } from 'lucide-react';

/*
 * The prototype's shell glyphs come from a lucide snapshot older than the installed
 * lucide-react: upstream redesigned house (gained a door), sliders-horizontal,
 * activity, bell, search (lens grew to r8), building-2, book-open and moon. The
 * prototype is the acceptance standard, so these icons keep the prototype's own path
 * data, built through lucide's public createLucideIcon factory — same stroke, caps,
 * joins and 24px view box as every other lucide icon in the app. Glyphs that still
 * match the prototype exactly (folder, users, inbox, chevron-right, sun) are imported
 * from lucide-react directly in AppShell.
 */

const glyph = (name: string, node: Array<[string, Record<string, string | number>]>) => (
  createLucideIcon(name, node as never) as unknown as LucideIcon
);

/** Prototype ICONS.home: roof line and walls without the door lucide later added. */
export const ShellWorkbenchIcon = glyph('shell-workbench', [
  ['path', { d: 'M3 10.5 12 3l9 7.5' }],
  ['path', { d: 'M5 9.5V20a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9.5' }],
]);

/** Prototype ICONS.inbox: lucide's own shape, but as the prototype's exact path nodes
 *  (lucide renders the tray as a polyline and rounds the arc slightly differently). */
export const ShellInboxIcon = glyph('shell-inbox', [
  ['path', { d: 'M22 12h-6l-2 3h-4l-2-3H2' }],
  ['path', { d: 'M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z' }],
]);

/** Prototype ICONS.users: the exact node order and arc values lucide later re-rounded. */
export const ShellMembersIcon = glyph('shell-members', [
  ['path', { d: 'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2' }],
  ['circle', { cx: 9, cy: 7, r: 4 }],
  ['path', { d: 'M22 21v-2a4 4 0 0 0-3-3.87' }],
  ['path', { d: 'M16 3.13a4 4 0 0 1 0 7.75' }],
]);

/** Prototype ICONS.book: the pre-redesign open book. */
export const ShellLessonsIcon = glyph('shell-lessons', [
  ['path', { d: 'M12 7v14' }],
  ['path', { d: 'M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3Z' }],
]);

/** Prototype ICONS.activity: the pre-redesign pulse line. */
export const ShellActivityIcon = glyph('shell-activity', [
  ['path', { d: 'M22 12h-4l-3 9L9 3l-3 9H2' }],
]);

/** Prototype ICONS.sliders: three rows with round knobs, not lucide's tick marks. */
export const ShellSettingsIcon = glyph('shell-settings', [
  ['path', { d: 'M4 6h8' }],
  ['path', { d: 'M16 6h4' }],
  ['path', { d: 'M4 12h2' }],
  ['path', { d: 'M10 12h10' }],
  ['path', { d: 'M4 18h10' }],
  ['path', { d: 'M18 18h2' }],
  ['circle', { cx: 14, cy: 6, r: 2 }],
  ['circle', { cx: 8, cy: 12, r: 2 }],
  ['circle', { cx: 16, cy: 18, r: 2 }],
]);

/** Prototype ICONS.bell: the pre-redesign bell with a flat lip. */
export const ShellBellIcon = glyph('shell-bell', [
  ['path', { d: 'M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9' }],
  ['path', { d: 'M10.3 21a1.94 1.94 0 0 0 3.4 0' }],
]);

/** Prototype ICONS.search: the classic lens (r7) lucide later enlarged to r8. */
export const ShellSearchIcon = glyph('shell-search', [
  ['circle', { cx: 11, cy: 11, r: 7 }],
  ['path', { d: 'm21 21-4.3-4.3' }],
]);

/** Prototype ICONS.building: the pre-redesign office block. */
export const ShellBuildingIcon = glyph('shell-building', [
  ['path', { d: 'M6 22V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v18Z' }],
  ['path', { d: 'M6 12H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h2' }],
  ['path', { d: 'M18 9h2a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-2' }],
  ['path', { d: 'M10 6h4' }],
  ['path', { d: 'M10 10h4' }],
  ['path', { d: 'M10 14h4' }],
  ['path', { d: 'M10 18h4' }],
]);

/** Prototype ICONS.moon: the crescent lucide later smoothed into a double arc. */
export const ShellMoonIcon = glyph('shell-moon', [
  ['path', { d: 'M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z' }],
]);
