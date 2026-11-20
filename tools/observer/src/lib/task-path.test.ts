import { describe, expect, it } from 'vitest';
import { taskPath } from './task-path';

describe('taskPath', () => {
  it('uses the designed task route with the project as its scope', () => {
    expect(taskPath('demo', 'work-a')).toBe('/tasks/work-a?scope=demo');
  });

  it('encodes both route spaces without exposing them in link text', () => {
    expect(taskPath('project /?&=', 'work/id ?')).toBe('/tasks/work%2Fid%20%3F?scope=project%20%2F%3F%26%3D');
  });
});
