"""Run a bounded command with contained descendants and a runner-owned log."""
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time


class _WindowsJob:
    """Assign a bootstrap before it starts the command; kill the complete job."""

    def __init__(self):
        import ctypes as c
        from ctypes import wintypes as w

        class BasicLimits(c.Structure):
            _fields_ = [('process_time', c.c_longlong), ('job_time', c.c_longlong),
                        ('flags', w.DWORD), ('min_working_set', c.c_size_t),
                        ('max_working_set', c.c_size_t), ('active_limit', w.DWORD),
                        ('affinity', c.c_size_t), ('priority', w.DWORD), ('scheduling', w.DWORD)]

        class IoCounters(c.Structure):
            _fields_ = [(name, c.c_ulonglong) for name in
                        ('read_ops', 'write_ops', 'other_ops', 'read_bytes', 'write_bytes', 'other_bytes')]

        class ExtendedLimits(c.Structure):
            _fields_ = [('basic', BasicLimits), ('io', IoCounters),
                        ('process_memory', c.c_size_t), ('job_memory', c.c_size_t),
                        ('peak_process_memory', c.c_size_t), ('peak_job_memory', c.c_size_t)]

        self.api = c.WinDLL('kernel32', use_last_error=True)
        self.api.CreateJobObjectW.argtypes = [c.c_void_p, w.LPCWSTR]
        self.api.CreateJobObjectW.restype = w.HANDLE
        self.api.SetInformationJobObject.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD]
        self.api.SetInformationJobObject.restype = w.BOOL
        self.api.TerminateJobObject.argtypes = [w.HANDLE, w.UINT]
        self.api.TerminateJobObject.restype = w.BOOL
        self.api.CloseHandle.argtypes = [w.HANDLE]
        self.api.CloseHandle.restype = w.BOOL
        self.handle = self.api.CreateJobObjectW(None, None)
        if not self.handle:
            raise c.WinError(c.get_last_error())
        limits = ExtendedLimits()
        limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not self.api.SetInformationJobObject(self.handle, 9, c.byref(limits), c.sizeof(limits)):
            error = c.WinError(c.get_last_error())
            self.close()
            raise error

    def start(self, command, **options):
        startup = subprocess.STARTUPINFO()
        startup.lpAttributeList = {'handle_list': [self.handle]}
        os.set_handle_inheritable(self.handle, True)
        try:
            return subprocess.Popen([sys.executable, str(Path(__file__).resolve()),
                                     '--windows-job', str(self.handle), *command],
                                    startupinfo=startup, close_fds=True, **options)
        finally:
            os.set_handle_inheritable(self.handle, False)

    def terminate(self):
        import ctypes
        if not self.api.TerminateJobObject(self.handle, 1):
            raise ctypes.WinError(ctypes.get_last_error())

    def close(self):
        if self.handle:
            self.api.CloseHandle(self.handle)
            self.handle = None


def _windows_bootstrap(handle, command):
    import ctypes as c
    from ctypes import wintypes as w
    api = c.WinDLL('kernel32', use_last_error=True)
    api.GetCurrentProcess.argtypes = []
    api.GetCurrentProcess.restype = w.HANDLE
    api.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]
    api.AssignProcessToJobObject.restype = w.BOOL
    api.CloseHandle.argtypes = [w.HANDLE]
    api.CloseHandle.restype = w.BOOL
    if not api.AssignProcessToJobObject(handle, api.GetCurrentProcess()):
        raise c.WinError(c.get_last_error())
    api.CloseHandle(handle)
    return subprocess.call(command)


def _terminate_family(process, job):
    if job is not None:
        job.terminate()
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def _positive_seconds(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
        raise ValueError('Process budgets must be finite positive seconds')
    return value


def run_command(command, *, cwd, log, timeout_seconds, cleanup_seconds, env=None):
    """Finalize only after the launcher is reaped and the output writer finishes.

    Cleanup also runs after normal exit, so an orphan cannot retain the log pipe.
    Failed containment or drainage is explicit and must stop subsequent gates.
    """
    _positive_seconds(timeout_seconds)
    _positive_seconds(cleanup_seconds)
    log = Path(log)
    log.touch(exist_ok=False)
    result = {'exit_code': None, 'timed_out': False, 'cleanup_complete': True,
              'log_finalized': True, 'timeout_seconds': timeout_seconds,
              'cleanup_timeout_seconds': cleanup_seconds,
              'process_boundary': 'windows_job' if os.name == 'nt' else 'posix_process_group'}
    process = reader = job = None
    errors = []

    def copy_output():
        try:
            with log.open('wb') as stream:
                while chunk := process.stdout.read1(65536):
                    stream.write(chunk)
                    stream.flush()
                os.fsync(stream.fileno())
        except Exception as error:
            errors.append('Output capture failed: ' + str(error))

    try:
        options = dict(cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if os.name == 'nt':
            job = _WindowsJob()
            process = job.start(list(map(str, command)), **options)
        else:
            process = subprocess.Popen(command, start_new_session=True, **options)
        reader = threading.Thread(target=copy_output, daemon=True)
        reader.start()
        try:
            result['exit_code'] = process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired:
            result['timed_out'] = True
    except Exception as error:
        errors.append('Command startup or wait failed: ' + str(error))
    finally:
        deadline = time.monotonic() + cleanup_seconds
        if process is not None:
            try:
                _terminate_family(process, job)
            except Exception as error:
                errors.append('Process-family termination failed: ' + str(error))
            try:
                if process.poll() is None:
                    process.kill()
                result['exit_code'] = process.wait(timeout=max(0, deadline - time.monotonic()))
            except Exception as error:
                errors.append('Launcher reap failed: ' + str(error))
        if job is not None:
            job.close()
        if reader is not None:
            reader.join(timeout=max(0, deadline - time.monotonic()))
            result['log_finalized'] = not reader.is_alive()
            if reader.is_alive():
                errors.append('Output writer did not finish within the cleanup budget')
            else:
                process.stdout.close()
        result['cleanup_complete'] = not errors and (process is None or process.poll() is not None)
    if errors:
        result['error'] = '; '.join(errors)
    result['passed'] = (result['exit_code'] == 0 and not result['timed_out']
                        and result['cleanup_complete'] and result['log_finalized'])
    return result


if __name__ == '__main__':
    if os.name != 'nt' or len(sys.argv) < 4 or sys.argv[1] != '--windows-job':
        raise SystemExit('This entry point is only a Windows job bootstrap')
    raise SystemExit(_windows_bootstrap(int(sys.argv[2]), sys.argv[3:]))
