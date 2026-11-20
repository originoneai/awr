/** The designed task route; the current placeholder can later become a full task page without changing callers. */
export function taskPath(project: string | null, workId: string): string {
  const task = `/tasks/${encodeURIComponent(workId)}`;
  // The cross-project inbox links a task without a scope; scoped pages keep their project.
  return project ? `${task}?scope=${encodeURIComponent(project)}` : task;
}
