import { useTranslation } from 'react-i18next';
import type { DerivedTaskStatus } from '../../core/overview/types';
import './display.css';

/**
 * The prototype's `.lamp` indicator (app.js stLamp/hLamp): a small state dot that is solid
 * for settled states and a thin ring for quiet states. Color never carries the meaning
 * alone; every tag also shows its word, and task tags add a hover sentence.
 */
export type TaskLampState = 'done' | 'running' | 'stalled' | 'blocked' | 'ready' | 'waiting' | 'record' | 'none';

/** Lamp states the prototype renders as a ring instead of a solid dot (RING in app.js). */
const RING_LAMPS = new Set<TaskLampState>(['ready', 'waiting', 'record', 'none']);

/** The prototype's tsTag mapping from a task status to its lamp state. */
const TASK_LAMP: Record<DerivedTaskStatus, TaskLampState> = {
  verified: 'done',
  claimed: 'record',
  returned: 'blocked',
  review: 'stalled',
  running: 'running',
  stalled: 'stalled',
  waiting: 'waiting',
  ready: 'ready',
  planned: 'waiting',
  unknown: 'none',
};

/** The prototype's tsTag mapping from a task status to its tag tone (null keeps the neutral pill). */
const TASK_TONE: Partial<Record<DerivedTaskStatus, 'ok' | 'risk' | 'off' | 'info'>> = {
  verified: 'ok',
  returned: 'off',
  review: 'risk',
  stalled: 'risk',
  running: 'info',
};

export function taskLampState(status: DerivedTaskStatus): TaskLampState {
  return TASK_LAMP[status];
}

export function StatusLamp({ lamp, live = false }: { lamp: TaskLampState; live?: boolean }) {
  return (
    <span
      className="obs-lamp"
      data-lamp={lamp}
      data-ring={RING_LAMPS.has(lamp) || undefined}
      data-live={live || undefined}
      aria-hidden="true"
    />
  );
}

export type HealthStatus = 'ok' | 'risk' | 'off' | 'none' | 'info';

/** The health grades reuse the lamp scale; 'none' stays the quiet ring. */
export function healthLampState(status: HealthStatus): TaskLampState {
  return status === 'ok' ? 'done'
    : status === 'info' ? 'running'
      : status === 'risk' ? 'stalled'
        : status === 'off' ? 'blocked'
          : 'none';
}

/** The prototype's HEALTH words: one rollup grade per project or goal. */
export function HealthTag({ status, label, meaning }: { status: HealthStatus; label?: string; meaning?: string }) {
  const { t } = useTranslation();
  const lamp = healthLampState(status);
  return (
    <span className="obs-tag" data-tone={status === 'none' ? undefined : status} title={meaning ?? t(`display.healthStatus.${status}.meaning`)}>
      <StatusLamp lamp={lamp} />
      {label ?? t(`display.healthStatus.${status}.label`)}
    </span>
  );
}

/**
 * One of the ten task statuses of the derivation vocabulary (nine plus unknown). The label
 * and hover sentence default to the shared vocabulary; a prototype page may supply its exact
 * localized wording without changing the shared status mapping.
 */
export function TaskStatusTag({ status, live = false, label, meaning }: {
  status: DerivedTaskStatus;
  live?: boolean;
  label?: string;
  meaning?: string;
}) {
  const { t } = useTranslation();
  const tone = TASK_TONE[status];
  return (
    <span className="obs-tag" data-tone={tone} title={meaning ?? t(`display.taskStatus.${status}.meaning`)}>
      <StatusLamp lamp={taskLampState(status)} live={live && status === 'running'} />
      {label ?? t(`display.taskStatus.${status}.label`)}
    </span>
  );
}
