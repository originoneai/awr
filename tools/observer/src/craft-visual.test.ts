import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('craft product visual signature', () => {
  it('uses antd steps for the delivery evidence signature rather than decorative generated imagery', () => {
    expect(readFileSync('src/pages/ProjectAcceptance.tsx', 'utf8')).toContain('<DeliverySteps');
    expect(readFileSync('src/components/DeliverySteps.tsx', 'utf8')).toContain('<Steps');
  });

  it('removes the decorative stock 403 illustration without hiding the authorization state', () => {
    const states = readFileSync('src/components/states.tsx', 'utf8');
    expect(states).toContain('data-testid="state-forbidden"');
    expect(states).toContain('status="info"');
    expect(states).toContain('icon={null}');
  });

  it('marks only verified digests with the stamp signature from the icon registry', () => {
    const artifact = readFileSync('src/components/ArtifactReader.tsx', 'utf8');
    const knowledge = readFileSync('src/pages/ProjectKnowledge.tsx', 'utf8');
    expect(artifact).toContain("status === 'verified' ? <StampIcon size={16} strokeWidth={1.5}");
    expect(artifact).toContain("import { StampIcon } from './icons/prototype-icons'");
    expect(knowledge).toContain("digestCheck === 'verified' ? <StampIcon size={16} strokeWidth={1.5}");
    expect(knowledge).toContain("import { StampIcon } from '../components/icons/prototype-icons'");
  });
});
