import { Copy, Terminal } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { PersonPendingItem } from '../../core/overview';
import { buildHandback, copyText, handbackVocabulary } from '../../lib/agent-handback';
import type { HandbackKind } from '../../lib/agent-handback';
import { DisplayIcon } from '../display';
import './workbench.css';

function handbackKind(item: PersonPendingItem): HandbackKind {
  if (item.kind === 'handoff') return 'assignment';
  if (item.kind === 'approval') return item.approval?.kind === 'lesson-confirmation' ? 'lesson' : 'plan-approval';
  return 'decision';
}

/** Browser-only handback for the first pending item; it performs no business mutation or request. */
export function AgentHandback({ item, projectName }: { item: PersonPendingItem; projectName: string }) {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<'idle' | 'copied' | 'failed'>('idle');
  const taskName = item.task?.title ?? item.approval?.title ?? null;
  const result = buildHandback({
    kind: handbackKind(item),
    projectName,
    taskName,
    question: item.wait?.what ?? null,
  }, {
    person: { agentConnections: [] },
    task: item.task ? { executionAgentName: item.task.agentName } : null,
  }, i18n.language);

  async function copy() {
    const copied = await copyText(result.sentence);
    if (copied.ok) {
      setStatus('copied');
      return;
    }
    const sentence = document.querySelector('[data-testid="agent-handback-sentence"]');
    if (sentence) {
      const selection = window.getSelection();
      const range = document.createRange();
      range.selectNodeContents(sentence);
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
    setStatus('failed');
  }

  return (
    <section className="obs-agent-handback" aria-label={t('workbench.home.handbackTitle')} data-testid="agent-handback">
      <div className="obs-agent-handback-title"><DisplayIcon icon={Terminal} />{t('workbench.home.handbackTitle')}</div>
      <p>{result.where}</p>
      <div className="obs-agent-handback-line">
        <span data-testid="agent-handback-sentence">{result.sentence}</span>
        <button type="button" onClick={copy}>
          <DisplayIcon icon={Copy} />{t('workbench.home.handbackCopy')}
        </button>
      </div>
      {status === 'idle' ? null : (
        <div role="status" className="obs-agent-handback-status">
          {t(status === 'copied' ? 'workbench.home.handbackCopied' : 'workbench.home.handbackCopyFailed')}
        </div>
      )}
      <div className="obs-agent-handback-note">{t('workbench.home.handbackReadOnly')}</div>
      <span className="obs-agent-handback-kind" aria-hidden="true">{handbackVocabulary(i18n.language).kind[handbackKind(item)]}</span>
    </section>
  );
}
