import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import {
  ArrowRight,
  BookOpen,
  CircleAlert,
  CircleCheck,
  CircleHelp,
  CircleX,
  Copy,
  Inbox as InboxIcon,
  Scale,
  Split,
  Terminal,
  type LucideIcon,
} from 'lucide-react';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { ObservationForbidden } from '../components/states';
import { buildHandback, copyText } from '../lib/agent-handback';
import { useInboxModel } from '../lib/inbox-queries';
import {
  inboxPendingLabel,
  inboxTaskPath,
  resolveInboxSelection,
  type InboxBasisItem,
  type InboxEntry,
  type InboxKind,
} from '../lib/inbox-model';
import './InboxPage.css';

/** Prototype KIND_ICON (src3/app.js): one icon per pending kind, stroke 1.5, 18px. */
const KIND_ICONS: Record<InboxKind, LucideIcon> = {
  answer: CircleHelp,
  decision: Scale,
  'plan-approval': Split,
  assignment: InboxIcon,
  lesson: BookOpen,
};

const BASIS_ICONS: Record<InboxBasisItem['tone'], LucideIcon | null> = {
  ok: CircleCheck,
  bad: CircleX,
  info: CircleAlert,
};

/** Untrusted titles beyond this many code points are clamped instead of wrapping the layout away. */
const TITLE_CLAMP_CODE_POINTS = 80;

function needsTitleClamp(title: string): boolean {
  return Array.from(title).length > TITLE_CLAMP_CODE_POINTS;
}

/**
 * The prototype's inbox page: a queue on the left, one focus on the right, and the
 * handback box that points back to the member's own Agent. The page performs no
 * business action of any kind — the only buttons are copy and navigation.
 */
export function InboxPage() {
  const { t, i18n } = useTranslation();
  const inbox = useInboxModel();
  const [searchParams, setSearchParams] = useSearchParams();
  const [copyState, setCopyState] = useState<'idle' | 'ok' | 'fallback'>('idle');
  const model = inbox.model;
  const requestedKey = searchParams.get('detail');
  const listReady = !inbox.loading && !inbox.forbidden && !inbox.allFailed && !inbox.projectsError;
  const selected = useMemo(
    () => (listReady ? resolveInboxSelection(model, requestedKey) : null),
    [listReady, model, requestedKey],
  );

  // URL state is clamped only when a link carried one: a bogus detail key resolves to
  // the first queue entry, and unrelated parameters are preserved.
  useEffect(() => {
    if (!listReady || requestedKey == null) return;
    const expected = selected?.key ?? '';
    if (requestedKey === expected) return;
    const next = new URLSearchParams(searchParams);
    if (expected) next.set('detail', expected);
    else next.delete('detail');
    setSearchParams(next, { replace: true });
  }, [listReady, selected, requestedKey, searchParams, setSearchParams]);

  useEffect(() => { setCopyState('idle'); }, [selected?.key]);

  if (inbox.forbidden) {
    return (
      <div className="inbox-page" data-testid="page-inbox">
        <ObservationForbidden />
      </div>
    );
  }

  const label = inboxPendingLabel(model);
  const countText = label ?? '—';
  const viewerName = model.viewer?.displayName ?? t('inbox.page.viewerUnknown');
  const oldest = model.entries.reduce<number | null>((current, entry) => (
    entry.durationDays == null ? current : current == null ? entry.durationDays : Math.max(current, entry.durationDays)
  ), null);
  const showList = listReady && model.count != null;

  function ageText(entry: InboxEntry): string {
    if (entry.durationDays == null) return t('inbox.page.ageUnknown');
    if (entry.durationDays === 0) return t('inbox.page.ageToday');
    return t('inbox.page.ageDays', { count: entry.durationDays });
  }

  function fromText(entry: InboxEntry): string {
    switch (entry.kind) {
      case 'answer':
        return entry.executionAgentName
          ? t('inbox.page.from.ownAgent', { agent: entry.executionAgentName })
          : t('inbox.page.from.ownTask');
      case 'decision':
        return entry.taskOwner?.displayName
          ? t('inbox.page.from.othersTask', { owner: entry.taskOwner.displayName })
          : t('inbox.page.from.othersTaskUnnamed');
      case 'assignment':
        return entry.fromPerson?.displayName
          ? t('inbox.page.from.assignment', { who: entry.fromPerson.displayName })
          : t('inbox.page.from.assignmentUnnamed');
      case 'lesson':
        return t('inbox.page.from.lesson');
      default:
        return t('inbox.page.from.planApproval');
    }
  }

  function descriptionText(entry: InboxEntry): string {
    const title = entry.title ?? t('inbox.page.titleMissing');
    switch (entry.kind) {
      case 'answer':
      case 'decision':
        return entry.taskName
          ? t('inbox.page.description.answer', { question: title, task: entry.taskName })
          : t('inbox.page.description.answerNoTask', { question: title });
      case 'assignment':
        return entry.fromPerson?.displayName
          ? t('inbox.page.description.assignment', { from: entry.fromPerson.displayName })
          : t('inbox.page.description.assignmentNoFrom');
      case 'lesson':
        return t('inbox.page.description.lesson', { title });
      default:
        return t('inbox.page.description.planApproval');
    }
  }

  function basisText(item: InboxBasisItem): string {
    switch (item.fact.fact) {
      case 'task':
        return t('inbox.page.basisTask', { title: item.fact.title });
      case 'reportAge': {
        const age = item.fact.days === 0
          ? t('inbox.page.ageToday')
          : t('inbox.page.reportAgeDays', { count: item.fact.days });
        return t('inbox.page.basisReportAge', { age });
      }
      case 'noReport':
        return t('inbox.page.basisNoReport');
      case 'independentStart':
        return t('inbox.page.basisIndependent');
      default:
        return t('inbox.page.basisBlocked', { count: item.fact.count });
    }
  }

  const handback = selected?.mine
    ? buildHandback({
      kind: selected.kind,
      projectName: selected.projectName,
      taskName: selected.taskName,
      question: selected.question,
    }, {
      // The page itself reads no connection list; without it the rules honestly
      // fall back to the any-Agent phrase instead of naming one.
      person: { agentConnections: [] },
      task: { executionAgentName: selected.executionAgentName },
    }, i18n.language)
    : null;

  async function copyLine() {
    if (!handback) return;
    const result = await copyText(handback.sentence);
    setCopyState(result.ok ? 'ok' : 'fallback');
  }

  function selectEntry(key: string) {
    const next = new URLSearchParams(searchParams);
    next.set('detail', key);
    setSearchParams(next);
  }

  const coverageText = model.count == null
    ? t('inbox.page.coverageUnread')
    : model.partial || model.failedReads > 0
      ? t('inbox.page.coveragePartial')
      : t('inbox.page.coverageComplete');

  const stateMessage = (() => {
    if (inbox.loading) {
      return <div className="inbox-state" role="status" data-testid="inbox-loading">{t('inbox.page.loading')}</div>;
    }
    if (inbox.projectsError || inbox.allFailed) {
      return <div className="inbox-state" data-testid="inbox-failed">{t('inbox.page.failed')}</div>;
    }
    if (model.viewerUnknown) {
      return <div className="inbox-state" data-testid="inbox-unknown">{t('inbox.page.unknownViewer')}</div>;
    }
    if (model.count == null) {
      return <div className="inbox-state" data-testid="inbox-unknown">{t('inbox.page.unknownRead')}</div>;
    }
    if (model.count === 0) {
      return <div className="inbox-state" data-testid="inbox-empty">{t('inbox.page.empty')}</div>;
    }
    return null;
  })();

  return (
    <div className="inbox-page" data-testid="page-inbox">
      <header className="inbox-head">
        <h1 className="inbox-title">{t('inbox.page.title')}</h1>
        <span className="inbox-sub">{viewerName}</span>
      </header>
      <p className="inbox-lead" data-testid="inbox-lead">
        <span className="inbox-lead-count" data-testid="inbox-queue-count">{countText}</span>
        {' '}
        {t('inbox.page.leadNoun')}
        <span>
          {model.count == null
            ? t('inbox.page.leadTailUnknownAge')
            : oldest != null
              ? t('inbox.page.leadTail', { oldest: t('inbox.page.oldestDays', { count: oldest }) })
              : t('inbox.page.leadTailUnknownAge')}
        </span>
      </p>
      {showList && (model.count ?? 0) > 0 && (model.partial || model.failedReads > 0) ? (
        <div className="inbox-partial" data-testid="inbox-partial">{t('inbox.page.partial')}</div>
      ) : null}
      {showList && model.count != null && model.count > 0 ? (
        <div className="inbox-md">
          <section className="inbox-queue" data-testid="inbox-queue" aria-label={t('inbox.page.queueLabel')}>
            {model.entries.map((entry) => {
              const Icon = KIND_ICONS[entry.kind];
              const title = entry.title ?? t('inbox.page.titleMissing');
              const current = entry.key === selected?.key;
              return (
                <button
                  key={entry.key}
                  type="button"
                  className="inbox-qrow"
                  data-testid="inbox-queue-row"
                  aria-current={current ? 'true' : undefined}
                  onClick={() => selectEntry(entry.key)}
                >
                  <span className="inbox-ki" aria-hidden><Icon size={18} strokeWidth={1.5} /></span>
                  <span className="inbox-qrow-body">
                    <span className="inbox-qrow-title" data-testid="inbox-queue-title" data-truncate={needsTitleClamp(title) || undefined}>{title}</span>
                    <span className="inbox-qrow-meta">{t(`inbox.page.kind.${entry.kind}`)} · {fromText(entry)}</span>
                  </span>
                  <span className="inbox-qrow-age">{ageText(entry)}</span>
                </button>
              );
            })}
          </section>
          {selected ? (
            <section className="inbox-focus" data-testid="inbox-focus" aria-label={t('inbox.page.focusLabel')}>
              <div className="inbox-focus-body">
                <div className="inbox-focus-kind">
                  <span className="inbox-kind-tag">{t(`inbox.page.kind.${selected.kind}`)}</span>
                </div>
                <h3
                  className="inbox-focus-title"
                  data-truncate={needsTitleClamp(selected.title ?? '') || undefined}
                >
                  {selected.title ?? t('inbox.page.titleMissing')}
                </h3>
                <div className="inbox-focus-from">{fromText(selected)} · {ageText(selected)}</div>
                <p className="inbox-focus-ask">{descriptionText(selected)}</p>
                <div className="inbox-sec" data-testid="inbox-basis">
                  <h4>{t('inbox.page.basisTitle')}</h4>
                  {selected.basis.length === 0 ? (
                    <p>{t('inbox.page.basisEmpty')}</p>
                  ) : (
                    <ul className="inbox-ev">
                      {selected.basis.map((item, index) => {
                        const Icon = BASIS_ICONS[item.tone];
                        return (
                          <li key={index} data-s={item.tone}>
                            {Icon ? <span className="inbox-ev-icon" aria-hidden><Icon size={18} strokeWidth={1.5} /></span> : null}
                            {basisText(item)}
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </div>
                <div className="inbox-sec" data-testid="inbox-impact">
                  <h4>{t('inbox.page.impactTitle')}</h4>
                  <p>
                    {selected.impactText
                      ?? (selected.impactBehindCount != null && selected.impactBehindCount > 0
                        ? t('inbox.page.impactBehind', { count: selected.impactBehindCount })
                        : t('inbox.page.impactEmpty'))}
                  </p>
                </div>
                <div className="inbox-sec inbox-related" data-testid="inbox-related">
                  {selected.taskId ? (
                    <Link className="inbox-link" data-testid="inbox-task-link" to={inboxTaskPath(selected.taskId)}>
                      {t('inbox.page.relatedTask')}
                      <ArrowRight size={14} strokeWidth={1.5} aria-hidden />
                    </Link>
                  ) : null}
                  {selected.kind === 'assignment' && selected.fromPerson ? (
                    <Link className="inbox-link" to={`/members/${encodeURIComponent(selected.fromPerson.id)}`}>
                      {t('inbox.page.relatedPerson', { who: selected.fromPerson.displayName ?? t('inbox.page.viewerUnknown') })}
                      <ArrowRight size={14} strokeWidth={1.5} aria-hidden />
                    </Link>
                  ) : null}
                  {selected.kind === 'lesson' ? (
                    <Link className="inbox-link" to="/lessons">
                      {t('inbox.page.relatedLesson')}
                      <ArrowRight size={14} strokeWidth={1.5} aria-hidden />
                    </Link>
                  ) : null}
                </div>
              </div>
              {selected.mine && handback ? (
                <div className="inbox-where" data-testid="inbox-handback">
                  <div className="inbox-where-head">
                    <span className="inbox-where-icon" aria-hidden><Terminal size={16} strokeWidth={1.5} /></span>
                    {t('inbox.page.handbackTitle')}
                  </div>
                  <p>{handback.where}</p>
                  <div className="inbox-sayline">
                    <span className="inbox-sayline-text" data-testid="inbox-handback-line">{handback.sentence}</span>
                    <button type="button" className="inbox-copy" data-testid="inbox-copy" onClick={() => { void copyLine(); }}>
                      <Copy size={14} strokeWidth={1.5} aria-hidden />
                      {t('inbox.page.copyLine')}
                    </button>
                  </div>
                  {copyState !== 'idle' ? (
                    <span className="inbox-copy-status" role="status" data-testid="inbox-copy-status">
                      {t(copyState === 'ok' ? 'inbox.page.copied' : 'inbox.page.copyFailed')}
                    </span>
                  ) : null}
                  <div className="inbox-hint">{t('inbox.page.hint')}</div>
                </div>
              ) : (
                <div className="inbox-where" data-testid="inbox-not-mine">
                  <div className="inbox-where-head">
                    <span className="inbox-where-icon" aria-hidden><CircleAlert size={16} strokeWidth={1.5} /></span>
                    {t('inbox.page.notMineTitle')}
                  </div>
                  <p>{t('inbox.page.notMineBody', { who: selected.recipient.displayName ?? t('inbox.page.viewerUnknown') })}</p>
                </div>
              )}
            </section>
          ) : null}
        </div>
      ) : stateMessage}
      <div className="inbox-pnote">
        <span data-testid="inbox-coverage">{coverageText}</span>
        <span className="inbox-freshness"><FreshnessBadge freshness={inbox.freshness} /></span>
      </div>
    </div>
  );
}
