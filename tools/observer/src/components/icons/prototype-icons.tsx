/*
 * The one and only icon registry of the app (AWR-PPO-069).
 *
 * The prototype (panorama src3/app.js, ICONS) is the acceptance standard for every
 * glyph: it harvests a lucide snapshot older than the installed lucide-react, so the
 * same-name imports differ (house gained a door, the search lens grew to r8, users was
 * re-rounded, bot/copy/unlock became rect nodes, the circle-check arm changed…). Every
 * prototype glyph is therefore rebuilt here through lucide's public createLucideIcon
 * factory with the prototype's own node data, verbatim. `PROTOTYPE_ICONS` keys are the
 * prototype's own names; the fixture test pins every node against the prototype table.
 *
 * Rules (docs/dev/reference/icons.md): pages and components import icons ONLY from this
 * file. A new icon is first looked up in PROTOTYPE_ICONS; only a glyph with no
 * prototype synonym goes into the app-only section below, with a comment for its use.
 *
 * The prototype paints every icon the same way (measured in the live prototype page):
 * viewBox 0 0 24 24, fill none, stroke currentColor, stroke-width 1.5, round caps and
 * joins, 16px by default, colour inherited from the text role. The wrapper below makes
 * those the defaults of every registry icon; callers may still pass size/strokeWidth.
 */
import { forwardRef } from 'react';
import { createLucideIcon, type LucideIcon, type LucideProps } from 'lucide-react';

export type { LucideIcon };

/** The prototype's icon() default box and stroke (app.js: size || 16, stroke-width 1.5). */
export const PROTOTYPE_ICON_SIZE = 16;
export const PROTOTYPE_ICON_STROKE_WIDTH = 1.5;

type GlyphNode = [string, Record<string, string | number>];

const glyph = (name: string, node: readonly GlyphNode[]): LucideIcon => {
  const Base = createLucideIcon(name, node as never);
  const PrototypeGlyph = forwardRef<SVGSVGElement, LucideProps>(
    ({ size = PROTOTYPE_ICON_SIZE, strokeWidth = PROTOTYPE_ICON_STROKE_WIDTH, ...props }, ref) => (
      <Base ref={ref} size={size} strokeWidth={strokeWidth} {...props} />
    ),
  );
  PrototypeGlyph.displayName = `PrototypeGlyph(${name})`;
  return PrototypeGlyph as unknown as LucideIcon;
};

/** Prototype ICONS.home: roof line and walls without the door lucide later added. */
export const HomeIcon = glyph('home', [
  ['path', { d: 'M3 10.5 12 3l9 7.5' }],
  ['path', { d: 'M5 9.5V20a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9.5' }],
]);

/** Prototype ICONS.folder: lucide's own folder shape. */
export const FolderIcon = glyph('folder', [
  ['path', { d: 'M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z' }],
]);

/** Prototype ICONS.inbox: the tray head as one path, not lucide's current polyline node. */
export const InboxIcon = glyph('inbox', [
  ['path', { d: 'M22 12h-6l-2 3h-4l-2-3H2' }],
  ['path', { d: 'M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z' }],
]);

/** Prototype ICONS.book: the pre-redesign open book. */
export const BookIcon = glyph('book-open', [
  ['path', { d: 'M12 7v14' }],
  ['path', { d: 'M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3Z' }],
]);

/** Prototype ICONS.users: the node order and arc values lucide later re-rounded. */
export const UsersIcon = glyph('users', [
  ['path', { d: 'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2' }],
  ['circle', { cx: 9, cy: 7, r: 4 }],
  ['path', { d: 'M22 21v-2a4 4 0 0 0-3-3.87' }],
  ['path', { d: 'M16 3.13a4 4 0 0 1 0 7.75' }],
]);

/** Prototype ICONS.activity: the pre-redesign pulse line. */
export const ActivityIcon = glyph('activity', [
  ['path', { d: 'M22 12h-4l-3 9L9 3l-3 9H2' }],
]);

/** Prototype ICONS.sliders: three rows with round knobs, not lucide's tick marks. */
export const SlidersIcon = glyph('sliders-horizontal', [
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

/** Prototype ICONS.search: the classic lens (r7) lucide later enlarged to r8. */
export const SearchIcon = glyph('search', [
  ['circle', { cx: 11, cy: 11, r: 7 }],
  ['path', { d: 'm21 21-4.3-4.3' }],
]);

/** Prototype ICONS.bell: the pre-redesign bell with a flat lip. */
export const BellIcon = glyph('bell', [
  ['path', { d: 'M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9' }],
  ['path', { d: 'M10.3 21a1.94 1.94 0 0 0 3.4 0' }],
]);

/** Prototype ICONS.chev: chevron right, lucide's own shape. */
export const ChevronRightIcon = glyph('chevron-right', [
  ['path', { d: 'm9 18 6-6-6-6' }],
]);

/** Prototype ICONS.arrow: arrow right, lucide's own shape. */
export const ArrowRightIcon = glyph('arrow-right', [
  ['path', { d: 'M5 12h14' }],
  ['path', { d: 'm12 5 7 7-7 7' }],
]);

/** Prototype ICONS.check: check mark, lucide's own shape. */
export const CheckIcon = glyph('check', [
  ['path', { d: 'M20 6 9 17l-5-5' }],
]);

/** Prototype ICONS.okc: circle check with the prototype's short check arm. */
export const CircleCheckIcon = glyph('circle-check', [
  ['circle', { cx: 12, cy: 12, r: 10 }],
  ['path', { d: 'm9 12 2 2 4-4' }],
]);

/** Prototype ICONS.badc: circle cross, lucide's own shape. */
export const CircleXIcon = glyph('circle-x', [
  ['circle', { cx: 12, cy: 12, r: 10 }],
  ['path', { d: 'm15 9-6 6' }],
  ['path', { d: 'm9 9 6 6' }],
]);

/** Prototype ICONS.dot: circle information (info), lucide's own shape. */
export const InfoIcon = glyph('info', [
  ['circle', { cx: 12, cy: 12, r: 10 }],
  ['path', { d: 'M12 16v-4' }],
  ['path', { d: 'M12 8h.01' }],
]);

/** Prototype ICONS.clock: clock face, lucide's own shape. */
export const ClockIcon = glyph('clock', [
  ['circle', { cx: 12, cy: 12, r: 10 }],
  ['path', { d: 'M12 6v6l4 2' }],
]);

/** Prototype ICONS.ring: plain circle, lucide's own shape. */
export const CircleIcon = glyph('circle', [
  ['circle', { cx: 12, cy: 12, r: 10 }],
]);

/** Prototype ICONS.sun: sun, lucide's own shape. */
export const SunIcon = glyph('sun', [
  ['circle', { cx: 12, cy: 12, r: 4 }],
  ['path', { d: 'M12 2v2' }],
  ['path', { d: 'M12 20v2' }],
  ['path', { d: 'm4.93 4.93 1.41 1.41' }],
  ['path', { d: 'm17.66 17.66 1.41 1.41' }],
  ['path', { d: 'M2 12h2' }],
  ['path', { d: 'M20 12h2' }],
  ['path', { d: 'm6.34 17.66-1.41 1.41' }],
  ['path', { d: 'm19.07 4.93-1.41 1.41' }],
]);

/** Prototype ICONS.moon: the crescent lucide later smoothed into a double arc. */
export const MoonIcon = glyph('moon', [
  ['path', { d: 'M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z' }],
]);

/** Prototype ICONS.building: the pre-redesign office block. */
export const BuildingIcon = glyph('building-2', [
  ['path', { d: 'M6 22V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v18Z' }],
  ['path', { d: 'M6 12H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h2' }],
  ['path', { d: 'M18 9h2a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-2' }],
  ['path', { d: 'M10 6h4' }],
  ['path', { d: 'M10 10h4' }],
  ['path', { d: 'M10 14h4' }],
  ['path', { d: 'M10 18h4' }],
]);

/** Prototype ICONS.bot: the head drawn as a path, not lucide's current rect node. */
export const BotIcon = glyph('bot', [
  ['path', { d: 'M12 8V4H8' }],
  ['path', { d: 'M6 8h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2Z' }],
  ['path', { d: 'M2 14h2' }],
  ['path', { d: 'M20 14h2' }],
  ['path', { d: 'M15 13v2' }],
  ['path', { d: 'M9 13v2' }],
]);

/** Prototype ICONS.up: arrow up, lucide's own shape. */
export const ArrowUpIcon = glyph('arrow-up', [
  ['path', { d: 'm5 12 7-7 7 7' }],
  ['path', { d: 'M12 19V5' }],
]);

/** Prototype ICONS.down: arrow down, lucide's own shape. */
export const ArrowDownIcon = glyph('arrow-down', [
  ['path', { d: 'M12 5v14' }],
  ['path', { d: 'm19 12-7 7-7-7' }],
]);

/** Prototype ICONS.help: circle help, the pre-redesign question mark. */
export const CircleHelpIcon = glyph('circle-help', [
  ['circle', { cx: 12, cy: 12, r: 10 }],
  ['path', { d: 'M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3' }],
  ['path', { d: 'M12 17h.01' }],
]);

/** Prototype ICONS.split: the fork before lucide re-rounded its numbers. */
export const SplitIcon = glyph('split', [
  ['path', { d: 'M16 3h5v5' }],
  ['path', { d: 'M8 3H3v5' }],
  ['path', { d: 'M12 22v-8.3a4 4 0 0 0-1.17-2.83L3 3' }],
  ['path', { d: 'm15 9 6-6' }],
]);

/** Prototype ICONS.scale: the pre-redesign scales. */
export const ScaleIcon = glyph('scale', [
  ['path', { d: 'm16 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z' }],
  ['path', { d: 'm2 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z' }],
  ['path', { d: 'M7 21h10' }],
  ['path', { d: 'M12 3v18' }],
  ['path', { d: 'M3 7h2c2 0 5-1 7-2 2 1 5 2 7 2h2' }],
]);

/** Prototype ICONS.pen: the pre-redesign pen. */
export const PenIcon = glyph('pen', [
  ['path', { d: 'M12 20h9' }],
  ['path', { d: 'M16.4 3.6a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z' }],
]);

/** Prototype ICONS.unlock: the shackle body as a path, not lucide's current rect node. */
export const LockOpenIcon = glyph('lock-open', [
  ['path', { d: 'M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2Z' }],
  ['path', { d: 'M7 11V7a5 5 0 0 1 9.9-1' }],
]);

/** Prototype ICONS.layers: the pre-redesign stack. */
export const LayersIcon = glyph('layers', [
  ['path', { d: 'm12 2 9 5-9 5-9-5 9-5Z' }],
  ['path', { d: 'm3 12 9 5 9-5' }],
  ['path', { d: 'm3 17 9 5 9-5' }],
]);

/** Prototype ICONS.terminal: prompt first, underline second (lucide swapped the order). */
export const TerminalIcon = glyph('terminal', [
  ['path', { d: 'm4 17 6-6-6-6' }],
  ['path', { d: 'M12 19h8' }],
]);

/** Prototype ICONS.copy: the front square as a path, not lucide's current rect node. */
export const CopyIcon = glyph('copy', [
  ['path', { d: 'M10 8h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H10a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2Z' }],
  ['path', { d: 'M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2' }],
]);

/** Prototype ICONS.file: the square document head and its three text lines. */
export const FileTextIcon = glyph('file-text', [
  ['path', { d: 'M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z' }],
  ['path', { d: 'M14 2v4a2 2 0 0 0 2 2h4' }],
  ['path', { d: 'M16 13H8' }],
  ['path', { d: 'M16 17H8' }],
  ['path', { d: 'M10 9H8' }],
]);

/** Every glyph of the prototype's ICONS table, keyed by the prototype's own name. */
export const PROTOTYPE_ICONS = {
  home: HomeIcon,
  folder: FolderIcon,
  inbox: InboxIcon,
  book: BookIcon,
  users: UsersIcon,
  activity: ActivityIcon,
  sliders: SlidersIcon,
  search: SearchIcon,
  bell: BellIcon,
  chev: ChevronRightIcon,
  arrow: ArrowRightIcon,
  check: CheckIcon,
  okc: CircleCheckIcon,
  badc: CircleXIcon,
  dot: InfoIcon,
  clock: ClockIcon,
  ring: CircleIcon,
  sun: SunIcon,
  moon: MoonIcon,
  building: BuildingIcon,
  bot: BotIcon,
  up: ArrowUpIcon,
  down: ArrowDownIcon,
  help: CircleHelpIcon,
  split: SplitIcon,
  scale: ScaleIcon,
  pen: PenIcon,
  unlock: LockOpenIcon,
  layers: LayersIcon,
  terminal: TerminalIcon,
  copy: CopyIcon,
  file: FileTextIcon,
} as const;

export type PrototypeIconName = keyof typeof PROTOTYPE_ICONS;

/* ------------------------------------------------------------------ *
 * App-only glyphs: no synonym in the prototype's ICONS table. The node
 * data is copied verbatim from the installed lucide-react 1.66.0.
 * ------------------------------------------------------------------ */

/** Scope-switcher dropdown caret in the shell header; the prototype has no chevron-down. */
export const ChevronDownIcon = glyph('chevron-down', [
  ['path', { d: 'm6 9 6 6 6-6' }],
]);

/** Back navigation on detail headers and the workbench metric detail; the prototype has no left arrow. */
export const ArrowLeftIcon = glyph('arrow-left', [
  ['path', { d: 'm12 19-7-7 7-7' }],
  ['path', { d: 'M19 12H5' }],
]);

/** Verified-digest seal on evidence readers; the prototype's ICONS has no stamp (its brand seal is the logo asset). */
export const StampIcon = glyph('stamp', [
  ['path', { d: 'M14 13V8.5C14 7 15 7 15 5a3 3 0 0 0-6 0c0 2 1 2 1 3.5V13' }],
  ['path', { d: 'M20 15.5a2.5 2.5 0 0 0-2.5-2.5h-11A2.5 2.5 0 0 0 4 15.5V17a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1z' }],
  ['path', { d: 'M5 22h14' }],
]);

/** Project map reload action; the prototype has no refresh glyph. */
export const RefreshCwIcon = glyph('refresh-cw', [
  ['path', { d: 'M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8' }],
  ['path', { d: 'M21 3v5h-5' }],
  ['path', { d: 'M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16' }],
  ['path', { d: 'M8 16H3v5' }],
]);

/** Warning notices (Notice tone="warning"); the prototype has no triangle glyph. */
export const TriangleAlertIcon = glyph('triangle-alert', [
  ['path', { d: 'm21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3' }],
  ['path', { d: 'M12 9v4' }],
  ['path', { d: 'M12 17h.01' }],
]);
