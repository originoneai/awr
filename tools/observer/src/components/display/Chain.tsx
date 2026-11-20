import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { ChainConnector, ChainNode, DerivedTaskStatus } from '../../core/overview/types';
import { useCompactViewport } from '../../lib/viewport';
import { StatusLamp, healthLampState, taskLampState, type HealthStatus } from './StatusTag';
import './display.css';

/** One row of a root's waiting queue (.ql span in the prototype). */
export interface ChainQueueEntry {
  title: string;
  /** Whole days; null is unknown and renders without an age. */
  days: number | null;
}

/**
 * Presentation of one chain node. The chain data is the core's ChainNode; the caller maps
 * each node to these display strings, keeping every wording decision at the page layer.
 */
export interface ChainNodeView {
  kicker?: string;
  title: string;
  meta?: string | null;
  /** Task status lamp for task nodes; the root keeps its fixed red lamp. */
  status?: DerivedTaskStatus | null;
  /** Health lamp for commitment nodes (null renders no lamp). */
  health?: HealthStatus | null;
  /** Display name shown as the trailing avatar chip in the kicker row. */
  ownerName?: string | null;
  queue?: readonly ChainQueueEntry[];
  /** Queue footer counts; rendered from the shared vocabulary, not by the caller. */
  queueNote?: { queued: number; oldestDays: number | null } | null;
  progress?: { done: number; total: number } | null;
  /** Free-text footer for nodes without a queue or progress bar. */
  footNote?: string | null;
  /** The viewer marker badge (.you in the prototype); at most one node per chain. */
  you?: boolean;
  /** The hot outline the prototype draws while the matching rail card is hovered. */
  highlighted?: boolean;
  /** Whether the node opens something; defaults to the presence of an onOpen handler. */
  open?: boolean;
}

export interface ChainProps {
  /** Core chain nodes, in walk order; the first root node renders with the red marker. */
  nodes: readonly ChainNode[];
  /** Core connectors; each is drawn before the node its toNodeId points at. */
  connectors?: readonly ChainConnector[];
  present: (node: ChainNode) => ChainNodeView;
  onOpen?: (node: ChainNode) => void;
  /** The prototype's .why-head, inside the same dotted canvas as the diagram. */
  header?: ReactNode;
  /** Designed no-chain content; when supplied it replaces the empty diagram row. */
  empty?: ReactNode;
  /** The quiet sentence under the chain (.why-foot). */
  footer?: ReactNode;
}

/**
 * The prototype's blocker chain (.chain/.cn/.cx): root marked red on the left, task nodes
 * joined by connectors, commitments at the end. Nodes are buttons when they open something;
 * the whole diagram is read-only and computes nothing.
 */
export function Chain({ nodes, connectors, present, onOpen, header, empty, footer }: ChainProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();

  const rendered: ReactNode[] = [];
  nodes.forEach((node, index) => {
    if (index > 0) {
      const connector = connectors?.find((candidate) => candidate.toNodeId === node.id);
      rendered.push(
        <span
          key={`connector:${node.id}`}
          className="obs-chain-connector"
          data-kind={connector?.kind}
          data-hot={index === 1 || undefined}
        >
          <span className="obs-chain-connector-icon">
            {/* The prototype's own 28x12 thin arrow (connector() in app.js), the one
                sanctioned non-lucide glyph; review round 2, R3. */}
            <svg width="28" height="12" viewBox="0 0 28 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
              <path d="M1 6h25M21 1.5 26 6l-5 4.5" />
            </svg>
          </span>
          <span>{t('display.chain.connector')}</span>
        </span>,
      );
    }
    const view = present(node);
    const open = view.open ?? onOpen !== undefined;
    const lamp = node.root ? <StatusLamp lamp="blocked" />
      : view.health ? <StatusLamp lamp={healthLampState(view.health)} />
        : view.status ? <StatusLamp lamp={taskLampState(view.status)} />
          : null;
    const classes = ['obs-chain-node', node.root ? 'obs-chain-node-root' : null].filter(Boolean).join(' ');
    const content = (
      <>
        {view.you ? <span className="obs-chain-you">{t('display.chain.youHere')}</span> : null}
        <span className="obs-chain-kicker">
          {lamp}
          {view.kicker}
          {view.ownerName ? <span className="obs-chain-avatar" aria-hidden="true">{initialOf(view.ownerName)}</span> : null}
        </span>
        <span className="obs-chain-title">{view.title}</span>
        {view.meta ? <span className="obs-chain-meta">{view.meta}</span> : null}
        {view.queue === undefined || view.queue.length === 0 ? null : (
          <span className="obs-chain-queue">
            {view.queue.map((entry) => (
              <span key={entry.title} className="obs-chain-queue-row">
                {entry.title}
                {entry.days === null ? null : (
                  <em>{t('display.chain.days', { count: entry.days })}</em>
                )}
              </span>
            ))}
          </span>
        )}
        {view.queueNote ? (
          <span className="obs-chain-foot">
            {view.queueNote.oldestDays === null
              ? t('display.chain.queueNoteUnknownAge', { queued: view.queueNote.queued })
              : t('display.chain.queueNote', { queued: view.queueNote.queued, days: view.queueNote.oldestDays })}
          </span>
        ) : view.progress ? (
          <span className="obs-chain-foot">
            <span className="obs-chain-progress">
              <i style={{ width: progressPercent(view.progress) }} />
            </span>
            {t('display.chain.progress', { done: view.progress.done, total: view.progress.total })}
          </span>
        ) : view.footNote ? (
          <span className="obs-chain-foot">{view.footNote}</span>
        ) : null}
      </>
    );
    if (!open || onOpen === undefined) {
      rendered.push(
        <div key={node.id} className={classes} data-node-kind={node.kind} data-highlighted={view.highlighted || undefined}>{content}</div>,
      );
    } else {
      rendered.push(
        <button
          key={node.id}
          type="button"
          className={classes}
          data-node-kind={node.kind}
          data-highlighted={view.highlighted || undefined}
          onClick={() => onOpen(node)}
        >
          {content}
        </button>,
      );
    }
  });

  return (
    <div className="obs-chain-block" data-compact={compactViewport || undefined}>
      {header}
      {nodes.length === 0 && empty !== undefined ? empty : (
        <div className="obs-chain" role="group" aria-label={t('display.chain.groupLabel')} data-compact={compactViewport || undefined}>
          {rendered}
        </div>
      )}
      {footer === undefined ? null : <div className="obs-chain-footer">{footer}</div>}
    </div>
  );
}

/** The prototype avatar takes the first character of the display name; groups keep their word. */
function initialOf(name: string): string {
  return name.slice(0, 1).toUpperCase();
}

function progressPercent(progress: { done: number; total: number }): string {
  if (progress.total <= 0) return '0%';
  return `${Math.round((100 * progress.done) / progress.total)}%`;
}
