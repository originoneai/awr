#!/usr/bin/env python3
"""Execute marked guide commands against a built CLI in disposable projects."""
import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
DOCUMENTS = [ROOT / 'docs/series/quickstart.md', ROOT / 'docs/series/daily-workflow.md']
TIMEOUT_SECONDS = 60


class DocCommandError(RuntimeError):
    """A guide command, output assertion or overall time budget failed."""


@dataclass
class Block:
    name: str
    line: int
    text: str
    expected: str | None = None


def extract_blocks(path):
    lines = path.read_text(encoding='utf-8').splitlines()
    blocks = []
    names = set()
    index = 0
    while index < len(lines):
        marker = re.fullmatch(r'<!-- doc-test: (.+?) -->', lines[index].strip())
        if marker:
            name = marker[1]
            if name in names:
                raise DocCommandError(f'{path}:{index + 1}: duplicate doc-test name: {name}')
            names.add(name)
            index += 1
            while index < len(lines) and not lines[index].strip():
                index += 1
            if index >= len(lines) or lines[index].strip() not in ('```sh', '```bash'):
                raise DocCommandError(f'{path}:{index + 1}: doc-test requires a shell fence')
            start = index + 1
            index = start
            while index < len(lines) and lines[index].strip() != '```':
                index += 1
            if index == len(lines) or not any(line.strip() for line in lines[start:index]):
                raise DocCommandError(f'{path}:{start + 1}: empty or unclosed doc-test block')
            block = Block(name, start + 1, '\n'.join(lines[start:index]) + '\n')
            blocks.append(block)
            following = index + 1
            while following < len(lines) and not lines[following].strip():
                following += 1
            if following < len(lines):
                expected = re.fullmatch(r'<!-- expect: (.+?) -->', lines[following].strip())
                if expected:
                    block.expected = expected[1]
        elif lines[index].startswith('```'):
            # Literal marker examples inside unmarked fences are documentation.
            index += 1
            while index < len(lines) and not lines[index].startswith('```'):
                index += 1
        index += 1
    if not blocks:
        raise DocCommandError(f'{path}:1: no doc-test blocks found')
    return blocks


def execute(command, *, cwd, env, deadline, label):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise DocCommandError(f'{label}: timeout: overall documentation budget exhausted')
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, encoding='utf-8',
                               start_new_session=True)
    try:
        output, _ = process.communicate(timeout=remaining)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        output, _ = process.communicate()
        raise DocCommandError(f'{label}: timeout: overall documentation budget exhausted\n{output}') from None
    if process.returncode:
        raise DocCommandError(f'{label}: command exited {process.returncode}\n{output}')
    return output


def run_documents(awr, documents=DOCUMENTS, *, timeout_seconds=TIMEOUT_SECONDS, deadline=None):
    started = time.monotonic()
    deadline = min(deadline or started + timeout_seconds, started + TIMEOUT_SECONDS)
    awr = Path(awr).resolve(strict=True)
    if not shutil.which('bash') or not shutil.which('jq'):
        raise DocCommandError('Documentation commands require bash and jq (provided by the Linux x64 CI image)')
    blocks_run = 0
    with tempfile.TemporaryDirectory(prefix='awr-doc-commands-') as directory:
        temporary = Path(directory)
        binaries = temporary / 'bin'
        binaries.mkdir()
        (binaries / 'awr').symlink_to(awr)
        for number, document in enumerate(documents):
            document = Path(document)
            blocks = extract_blocks(document)
            project = temporary / f'project-{number}'
            project.mkdir()
            for name in ('project.toml', 'GOALS.md', 'PLAN.md', 'RULES.md', 'work-ledger.yaml'):
                source = ROOT / 'examples' / ('codex' if name == 'work-ledger.yaml' else 'basic') / name
                text = source.read_text(encoding='utf-8').replace('EXAMPLE-001', 'INTAKE-001')
                (project / name).write_text(text, encoding='utf-8')
            state = temporary / f'user-state-{number}'
            state.mkdir()
            env = dict(os.environ, AWR_LOCAL_WEB='0', AWR_LOCAL_HOME=str(state),
                       PATH=str(binaries) + os.pathsep + os.environ.get('PATH', ''),
                       TMPDIR=str(temporary), AWR_PROJECT=str(project))
            execute([str(awr), '--project', str(project), 'init', '--manifest', 'project.toml', '--accept'],
                    cwd=project, env=env, deadline=deadline, label=f'{document}:1: fixture initialization')
            script = ['set -Eeuo pipefail', """trap 'DOC_ERROR_LINE=$LINENO DOC_ERROR_COMMAND=$BASH_COMMAND
for ((DOC_FRAME=1; DOC_FRAME < ${#BASH_SOURCE[@]}; DOC_FRAME++)); do
  if [[ ${BASH_SOURCE[DOC_FRAME]} == "$DOC_CODE" ]]; then
    DOC_ERROR_LINE=${BASH_LINENO[DOC_FRAME-1]}
    break
  fi
done
printf "%s:%s: command failed: %s\\n" "$DOC_FILE" "$((DOC_START + DOC_ERROR_LINE - 1))" "$DOC_ERROR_COMMAND" >&2
exit 1' ERR"""]
            completed = temporary / f'completed-{number}'
            for index, block in enumerate(blocks):
                code = temporary / f'block-{number}-{index}.sh'
                log = temporary / f'block-{number}-{index}.out'
                code.write_text(block.text, encoding='utf-8')
                script.extend([f'DOC_FILE={shlex.quote(str(document))}', f'DOC_START={block.line}',
                               f'DOC_CODE={shlex.quote(str(code))}',
                               f'printf "%s\\n" {shlex.quote(f"Running {document}:{block.line} ({block.name})")}',
                               f'source {shlex.quote(str(code))} > {shlex.quote(str(log))}',
                               f'cat {shlex.quote(str(log))}'])
                if block.expected is not None:
                    script.extend([f'DOC_OUTPUT=$(< {shlex.quote(str(log))})',
                                   f'if [[ "$DOC_OUTPUT" != *{shlex.quote(block.expected)}* ]]; then',
                                   f'  printf "%s\\n" {shlex.quote(f"{document}:{block.line}: expected output substring: {block.expected}")} >&2',
                                   '  exit 1', 'fi'])
                script.append(f'printf "%s\\n" {shlex.quote(block.name)} >> {shlex.quote(str(completed))}')
            driver = temporary / f'document-{number}.sh'
            driver.write_text('\n'.join(script) + '\n', encoding='utf-8')
            print(execute(['bash', str(driver)], cwd=project, env=env, deadline=deadline,
                          label=f'{document}:1'), end='')
            if not completed.exists() or completed.read_text(encoding='utf-8').splitlines() != [b.name for b in blocks]:
                raise DocCommandError(f'{document}:1: shell exited before every doc-test block completed')
            blocks_run += len(blocks)
    if time.monotonic() >= deadline:
        raise DocCommandError('timeout: overall documentation budget exhausted')
    return {'passed': True, 'documents': len(documents), 'blocks': blocks_run,
            'duration_seconds': round(time.monotonic() - started, 3)}


def main():
    started = time.monotonic()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--awr', type=Path, required=True, help='Built awr executable')
    parser.add_argument('--doc', type=Path, action='append', help='Override the two default guides')
    parser.add_argument('--self-test', action='store_true', help='Also run positive and negative runner contracts')
    args = parser.parse_args()
    deadline = started + TIMEOUT_SECONDS
    try:
        if args.self_test:
            print(execute([sys.executable, str(Path(__file__).with_name('test_run_doc_commands.py')),
                           '--awr', str(args.awr.resolve())], cwd=ROOT, env=os.environ.copy(),
                          deadline=deadline, label='Documentation runner tests'), end='')
        result = run_documents(args.awr, args.doc or DOCUMENTS, deadline=deadline)
        result['duration_seconds'] = round(time.monotonic() - started, 3)
        print(json.dumps(result))
        return 0
    except (DocCommandError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
