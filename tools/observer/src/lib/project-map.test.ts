import { describe, expect, it } from 'vitest';
import page from 'awr-progress-map/project-map/page.js';
import interaction from 'awr-progress-map/project-map/interact.js';
import en from 'awr-progress-map/locales/en.js';
import zh from 'awr-progress-map/locales/zh-CN.js';
import { teamProjectSnapshot, type GraphWork, type TeamGraphResponse } from './project-map';

function graph(): TeamGraphResponse {
  const work = (id: string, key: string, dependencies: string[]): GraphWork => ({
    work_id: id, external_key: key, workstream_id: 'development', title: key,
    state: 'planned', owner: null, last_event_at: null, dependencies_complete: true,
    required_dependencies: dependencies, claims: [],
  });
  return {
    ok: true, project: 'portal', read_at_unix_ms: 1_790_000_000_000,
    as_of: { project_revision: '12', source_snapshot_id: 'source-12', coordinator_epoch: 'current' },
    workstreams: [{ id: 'development', title: 'Development' }],
    items: [work('api', 'API-001', []), work('ui', 'UI-001', ['api'])],
  };
}

describe('Team projection into the shared project maps', () => {
  it.each([
    '让团队成员通过统一入口按权限完成任务规划、开发与验收并持续交付',
    'Let every team member plan, develop, review and deliver work through the shared project',
    'UninterruptedGroupName'.repeat(6),
  ])('wraps the complete group name in a fixed-width header and moves progress below it (%s)', (title) => {
    const response = graph();
    response.workstreams[0].title = title;
    response.workstreams.push({ id: 'review', title: 'Review' });
    response.items[1].workstream_id = 'review';
    response.items[1].required_dependencies = [];
    const rendered = page.renderProjectMap(teamProjectSnapshot(response), undefined, { t: page.makeT({ en }, 'en'), lang: 'en', interactive: true, productOnly: true });
    const container = document.createElement('div'); container.innerHTML = rendered.body;
    const headers = [...container.querySelectorAll('.pm-fold[data-lane]')];
    const header = headers.find(element => element.querySelector('title')?.textContent === title)!;
    expect(header.querySelector('title')?.textContent).toBe(title);
    const lines = [...header.querySelectorAll('text')].map(element => element.textContent ?? '');
    expect(lines.length).toBeGreaterThan(2);
    expect(lines.join('').replace(/\s/g, '')).toBe(title.replace(/\s/g, ''));
    const headerBottom = Math.max(...headers.map(element => {
      const rect = element.querySelector('rect')!;
      expect(Number(rect.getAttribute('width'))).toBe(214);
      return Number(rect.getAttribute('y')) + Number(rect.getAttribute('height'));
    }));
    const overview = header.closest('svg')!;
    const bars = [...overview.querySelectorAll('clipPath[id^="overview-bar-"] rect')];
    expect(bars).toHaveLength(2);
    expect(bars.map(element => Number(element.getAttribute('y')))).toEqual([headerBottom + 10, headerBottom + 10]);
    for (const card of overview.querySelectorAll('.pm-card > rect')) {
      expect(Number(card.getAttribute('y'))).toBeGreaterThan(headerBottom + 38);
    }
  });
  it('uses the registered project and workstream names when no goal was recorded', () => {
    const response = graph();
    response.workstreams[0].title = 'Customer portal delivery';
    const snapshot = teamProjectSnapshot(response, 'en-US', 'Customer portal');
    const rendered = page.renderProjectMap(snapshot, undefined, { t: page.makeT({ en }, 'en'), lang: 'en', interactive: false, productOnly: true });
    expect(snapshot.project).toMatchObject({ name: 'Customer portal' });
    expect(rendered.body).toContain('Customer portal delivery');
    expect(rendered.body).not.toMatch(/Work group \d/);
  });

  it('keeps the responsible owner distinct from an active claimant', () => {
    const response = graph();
    response.items[0].owner = 'Project owner';
    response.items[0].claims = [{ id: 'claim', actor_id: 'actor', actor_name: 'Executing agent', session_id: 'session',
      acquired_at_unix_ms: response.read_at_unix_ms, expires_at_unix_ms: response.read_at_unix_ms + 10000 }];
    const rendered = page.renderProjectMap(teamProjectSnapshot(response), undefined, { t: page.makeT({ en }, 'en'), lang: 'en', interactive: false, productOnly: true });
    const model = rendered.model as { nodes: Map<string, { owner: string }> };
    expect(model.nodes.get('API-001')?.owner).toBe('Project owner');
  });
  it.each(['en', 'zh-CN'])('keeps generated group summaries and completed-work previews in product language in %s', (language) => {
    const response = graph();
    response.items = Array.from({ length: 14 }, (_, index) => ({
      ...response.items[0], work_id: `internal-${index}`, external_key: `INTERNAL-${String(index + 1).padStart(3, '0')}`,
      title: `Review document ${index + 1}`, state: index < 4 ? 'completed' : 'planned', required_dependencies: [],
      goals: ['document-review'], goal_titles: [{ id: 'document-review', title: 'Project document review' }],
    }));
    response.items[4].state = 'in_progress';
    response.items[4].last_event_at = response.read_at_unix_ms;
    response.items[4].claims = [{ id: 'internal-claim', actor_id: 'reviewer', actor_name: 'Reviewer', session_id: 'internal-session', acquired_at_unix_ms: response.read_at_unix_ms, expires_at_unix_ms: response.read_at_unix_ms + 100000 }];
    const snapshot = teamProjectSnapshot(response);
    const translate = page.makeT({ en, 'zh-CN': zh }, language);
    const rendered = page.renderProjectMap(snapshot, undefined, { t: translate, lang: language, interactive: true, productOnly: true });
    const container = document.createElement('div');
    container.innerHTML = rendered.body;
    const text = container.textContent ?? '';
    expect(text).toContain('Review document');
    expect(text).toContain('Project document review');
    expect(text).not.toMatch(/Work group \d|工作组 \d/);
    expect(text.match(/INTERNAL|internal-session|internal-claim|display configuration|展示配置|传递约简|revision|node\./)?.[0]).toBeUndefined();
    expect(text).not.toMatch(/WORK \d|INTERNAL \d/);
    expect(container.querySelector('[data-pm-action="copy-command"]')).toBeNull();
  });

  it('keeps map cards, tables, accessibility labels and opened details free of internal identifiers', () => {
    const response = graph();
    response.items[0].title = 'Build the sign-in service';
    response.items[1].title = 'Connect the sign-in page';
    response.items[0].claims = [{ id: 'internal-claim', actor_id: '01M6AVQ870N7MBRJ5MPCN79T55', actor_name: null, session_id: 'internal-session', acquired_at_unix_ms: 1790000000000, expires_at_unix_ms: 1790001000000 }];
    const snapshot = teamProjectSnapshot(response);
    const translate = page.makeT({ en }, 'en');
    const rendered = page.renderProjectMap(snapshot, undefined, { t: translate, lang: 'en', interactive: true, productOnly: true });
    const container = document.createElement('div');
    container.innerHTML = rendered.body;
    document.body.appendChild(container);
    const mounted = interaction.mount({ container, snapshot, model: rendered.model, t: translate, productOnly: true });
    try {
      container.querySelector('.pm-card')?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      const descriptions = [...container.querySelectorAll('[title],[aria-label],title')].map(element => element.getAttribute('title') ?? element.getAttribute('aria-label') ?? element.textContent).join(' ');
      for (const raw of ['API-001', 'UI-001', 'internal-claim', 'internal-session', '01M6AVQ870N7MBRJ5MPCN79T55', 'source-12']) {
        expect(container.textContent).not.toContain(raw);
        expect(descriptions).not.toContain(raw);
      }
      expect(container.querySelector('[data-testid="technical-details"],.notes,[data-pm-action="copy-key"]')).toBeNull();
      expect(container.querySelectorAll('table th')).toHaveLength(6);
      expect(container.textContent).toContain('Build the sign-in service');
    } finally {
      mounted.destroy();
      container.remove();
    }
  });

  it('maps identities, dependencies and states without inventing missing data', () => {
    const response = graph();
    response.items[0].state = 'completed';
    const snapshot = teamProjectSnapshot(response);
    expect(snapshot.edges).toEqual([{ dependent: 'UI-001', prerequisite: 'API-001', required: true }]);
    expect(snapshot.nodes).toMatchObject([{ key: 'API-001', status: 'completed' }, { key: 'UI-001', status: 'planned' }]);
    expect(snapshot.doctor).toEqual({ ok: null, findings: [] });
    expect(snapshot.sessions).toEqual([]);
    expect(page.renderProjectMap(snapshot, undefined, {
      t: (key) => key, lang: 'en', interactive: true,
    }).body).toContain('<svg');
    expect(teamProjectSnapshot({ ...response, read_at_unix_ms: response.read_at_unix_ms + 1000 }).fingerprint).toBe(snapshot.fingerprint);
    expect(teamProjectSnapshot({ ...response, as_of: { ...response.as_of!, project_revision: '13' } }).fingerprint).not.toBe(snapshot.fingerprint);
  });

  it('refuses incomplete enumeration and conflicting display identities', () => {
    const response = graph();
    response.items[1].required_dependencies = ['unread'];
    expect(() => teamProjectSnapshot(response)).toThrow('incomplete');
    response.items[1].required_dependencies = [];
    response.items[1].external_key = response.items[0].external_key;
    expect(() => teamProjectSnapshot(response)).toThrow('identity');
  });

  it('preserves recorded claim times without inventing a session start', () => {
    const response = graph();
    response.items[0].claims = [{
      id: 'claim-api', actor_id: 'developer', actor_name: 'Developer', session_id: 'session-api',
      acquired_at_unix_ms: 1_789_999_000_000, expires_at_unix_ms: 1_790_001_000_000,
    }];
    const snapshot = teamProjectSnapshot(response);
    expect(snapshot.nodes).toMatchObject([{ claims: [{ acquired_at: 1_789_999_000_000 }] }, { claims: [] }]);
    expect(snapshot.sessions).toMatchObject([{ started_at: null, agent_id: 'Developer' }]);
  });

  it('maps unclaimed Team work to the backlog and preserves its original state', () => {
    const response = graph();
    response.items[0].state = 'unclaimed';
    expect((teamProjectSnapshot(response).nodes as object[])[0]).toMatchObject({ status: 'planned', source_status: 'unclaimed' });
    response.items[0].state = 'unsupported-future-state';
    expect((teamProjectSnapshot(response).nodes as object[])[0]).toMatchObject({ status: 'unknown', source_status: 'unsupported-future-state' });
  });

  it('keeps opaque source identifiers out of card subtitles while retaining their identity', () => {
    const response = graph();
    response.items[0].external_key = `WORK-${'b'.repeat(32)}`;
    expect(teamProjectSnapshot(response).nodes).toMatchObject([
      { key: 'UI-001' }, { key: response.items[0].external_key, display_key: '' },
    ]);
  });
});


describe('product-safe graph titles and names', () => {
  it.each(['zh-CN', 'en-US'])('keeps unsafe title and owner data out of the map and opened details in %s', (language) => {
    const response = graph();
    response.items[0].title = 'payload={"token":"synthetic-map-title"}';
    response.items[0].owner = 'client[secret]=synthetic-map-owner';
    response.items[0].claims = [{ id: 'private-claim', actor_id: 'private-actor', actor_name: 'GLM4.6',
      session_id: 'private-session', acquired_at_unix_ms: response.read_at_unix_ms, expires_at_unix_ms: response.read_at_unix_ms + 10000 }];
    response.items[1].title = 'Connect the sign-in page';
    const snapshot = teamProjectSnapshot(response, language);
    const translate = page.makeT({ en, 'zh-CN': zh }, language.startsWith('zh') ? 'zh-CN' : 'en');
    const rendered = page.renderProjectMap(snapshot, undefined, { t: translate, lang: language, interactive: true, productOnly: true });
    const container = document.createElement('div');
    container.innerHTML = rendered.body;
    document.body.appendChild(container);
    const mounted = interaction.mount({ container, snapshot, model: rendered.model, t: translate, productOnly: true });
    try {
      container.querySelector('.pm-card')?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      const text = [container.textContent, ...[...container.querySelectorAll('[title],[aria-label],title')]
        .flatMap((element) => [element.getAttribute('title'), element.getAttribute('aria-label'), element.textContent])].join(' ');
      expect(text).not.toMatch(/synthetic-map-title|synthetic-map-owner|payload|secret/);
      expect(text).toContain('Connect the sign-in page');
      expect(text).toContain(language === 'zh-CN' ? '某项工作' : 'A work item');
      expect(snapshot.nodes).toMatchObject([{ claims: [{ agent_id: 'GLM4.6' }] }, {}]);
    } finally { mounted.destroy(); container.remove(); }
  });
});
