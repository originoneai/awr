import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('craft product visual signature', () => {
  it('uses the existing evidence chain rather than decorative generated imagery', () => {
    expect(readFileSync('src/pages/ProjectAcceptance.tsx', 'utf8')).toContain('<EvidenceChain');
    expect(readFileSync('src/components/EvidenceChain.tsx', 'utf8')).toContain('<Steps');
  });

  it('marks only verified digests with the lucide stamp signature', () => {
    const artifact = readFileSync('src/components/ArtifactReader.tsx', 'utf8');
    const knowledge = readFileSync('src/pages/ProjectKnowledge.tsx', 'utf8');
    expect(artifact).toContain("status === 'verified' ? <Stamp size={16} strokeWidth={1.5}");
    expect(knowledge).toContain("digestCheck === 'verified' ? <Stamp size={16} strokeWidth={1.5}");
  });
});
