import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('craft layout rhythm', () => {
  it('uses the prototype master/detail outcome layout with a dominant status and evidence region', () => {
    const source = readFileSync('src/pages/ProjectAcceptance.tsx', 'utf8');
    expect(source).toContain('<DeliverySteps');
    expect(source).toContain('testId="acceptance-evidence-chain"');
    expect(source).toContain('lg={7}');
    expect(source).toContain('lg={17}');
    expect(source).toContain('acceptance.verificationTitle');
    expect(source).toContain('acceptance.reviewChanges');
    expect(source).toContain('acceptance.downstream');
  });

  it('uses asymmetric account and connection grids instead of equal card stacks', () => {
    const account = readFileSync('src/pages/AccountPage.tsx', 'utf8');
    const connect = readFileSync('src/pages/AgentConnectPage.tsx', 'utf8');
    expect(account).toContain('<Row gutter={[16, 16]}>');
    expect(account).toContain('lg={14}');
    expect(account).toContain('lg={10}');
    expect(connect).toContain('<Row gutter={[16, 16]}>');
    expect(connect).toContain('lg={14}');
    expect(connect).toContain('lg={10}');
  });

  it('keeps workline progress and recent activity as the main overview blocks', () => {
    const overview = readFileSync('src/pages/ProjectOverview.tsx', 'utf8');
    expect(overview).toContain('<Col xs={24} lg={16} style={{ minWidth: 0 }}>');
    expect(overview).toContain('<RecentActivity query={activity} />');
  });
});
