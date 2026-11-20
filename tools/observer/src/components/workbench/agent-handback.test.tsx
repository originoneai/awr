import { App as AntdApp } from 'antd';
import { render } from '@testing-library/react';
import { describe, it } from 'vitest';
import '../../i18n';
import { CraftThemeProvider } from '../CraftThemeProvider';
import { AgentHandback } from './AgentHandback';
import { expectSvgDrawsPrototypeGlyph } from '../icons/expect-prototype-glyph';
import type { PersonPendingItem } from '../../core/overview';

// The handback's two icons are prototype glyphs: the terminal marks "handle it in your
// Agent", the copy button marks the copyable sentence. Both are pinned to the registry,
// not to this component's own imports.

const item: PersonPendingItem = {
  kind: 'wait',
  task: null,
  wait: null,
  handoff: null,
  approval: null,
};

function renderHandback() {
  return render(
    <CraftThemeProvider>
      <AntdApp>
        <AgentHandback item={item} projectName="页面联调" />
      </AntdApp>
    </CraftThemeProvider>,
  );
}

describe('AgentHandback icons', () => {
  it('draws the handback terminal and the copy glyph as the prototype glyphs', () => {
    renderHandback();
    expectSvgDrawsPrototypeGlyph(
      document.querySelector('.obs-agent-handback-title svg'),
      'handback title marker',
      'terminal',
    );
    expectSvgDrawsPrototypeGlyph(
      document.querySelector('.obs-agent-handback button svg'),
      'handback copy button',
      'copy',
    );
  });
});
