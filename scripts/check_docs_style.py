#!/usr/bin/env python3
"""Check documentation style against a ratcheting baseline using the standard library.

R4 caps limitation sections at max(recorded count, 1) for existing pages.
"""
from __future__ import annotations

import argparse
import html
import io
import json
import math
import os
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
BASELINE = Path("docs/dev/style-baseline.json")
WORD = re.compile(r"[A-Za-z]+(?:['’][A-Za-z]+)?")
HAN = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff\U00020000-\U0002ebef]")
EN_NEGATIVE = re.compile(r"\b(?:not|never|cannot)\b|n['’]t\b", re.IGNORECASE)
ZH_NEGATIVE = re.compile(r"无法|禁止|不得|不|未")
HEADING = re.compile(r"^ {0,3}(#{1,6})\s+(.+?)(?:\s+#+)?\s*$")
LIMIT_HEADINGS = {"limits", "known limits", "boundaries", "限制", "边界"}
METRICS = ("density", "opening_words", "opening_negatives", "limit_sections")


def document_paths(root):
    patterns = ("README.md", "README.zh-CN.md", "docs/series/*.md",
                "docs/dev/reference/*.md", "docs/dev/integrations/*.md")
    return sorted({path.relative_to(root).as_posix()
                   for pattern in patterns for path in root.glob(pattern) if path.is_file()})


def visible_lines(text):
    """Preserve line numbers while hiding comments and fenced code examples."""
    text = re.sub(r"<!--.*?-->", lambda m: "\n" * m[0].count("\n"), text, flags=re.S)
    fence = None
    for number, line in enumerate(text.splitlines(), 1):
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})", line)
        if fence:
            if re.fullmatch(r" {0,3}" + re.escape(fence[0]) + "{" + str(len(fence)) + r",}\s*", line):
                fence = None
            yield number, ""
        elif marker:
            fence = marker[1]
            yield number, ""
        else:
            yield number, line


def heading_lines(text):
    lines = list(visible_lines(text))
    for index in range(len(lines) - 1):
        number, line = lines[index]
        underline = re.fullmatch(r" {0,3}(=+|-+)\s*", lines[index + 1][1])
        if line.strip() and not HEADING.match(line) and underline:
            level = "#" if underline[1].startswith("=") else "##"
            lines[index] = (number, f"{level} {line.strip()}")
            lines[index + 1] = (lines[index + 1][0], "")
    return lines


def opening(text):
    lines = heading_lines(text)
    start = next((i + 1 for i, (_, line) in enumerate(lines)
                  if (m := HEADING.match(line)) and len(m[1]) == 1), 0)
    paragraph = []
    first_line = 1
    for number, line in lines[start:]:
        prose_line = re.sub(r"<[^>]+>", "", line).strip()
        if (not prose_line or HEADING.match(line)
                or re.match(r"^\s*(?:!\[|[-*_]{3,}\s*$)", line)):
            if paragraph:
                break
            continue
        if not paragraph:
            first_line = number
        paragraph.append(prose_line)
    prose = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", " ".join(paragraph))
    prose = re.sub(r"<[^>]+>", "", prose)
    return first_line, prose


def is_chinese(text):
    return len(HAN.findall(text)) > len(WORD.findall(text))


def measure(text):
    chinese = is_chinese(text)
    negative = ZH_NEGATIVE if chinese else EN_NEGATIVE
    units = len((HAN if chinese else WORD).findall(text))
    _, intro = opening(text)
    return {
        "density": len(negative.findall(text)) * 1000 / units if units else 0.0,
        "opening_words": len(WORD.findall(intro)) + len(HAN.findall(intro)),
        "opening_negatives": len(negative.findall(intro)),
        "limit_sections": sum(1 for _, line in heading_lines(text)
                              if (m := HEADING.match(line)) and len(m[1]) > 1
                              and m[2].casefold() in LIMIT_HEADINGS),
    }


def density_applies(text):
    return is_chinese(text) or len(WORD.findall(text)) >= 300


def check_r1(path, text, current, recorded):
    if recorded is not None and density_applies(text) and current["density"] > recorded["density"] + 0.5:
        return [f'{path}:1: R1 density {current["density"]:.3f} exceeds baseline '
                f'{recorded["density"]:.3f} + 0.5 per 1,000 units']
    return []


def check_r2(path, text, current, recorded):
    if recorded is None and density_applies(text) and current["density"] > 8.0:
        return [f'{path}:1: R2 new-page density {current["density"]:.3f} exceeds 8.0 per 1,000 units']
    return []


def check_r3(path, text, current, recorded):
    limits = recorded if recorded is not None else {"opening_words": 60, "opening_negatives": 0}
    line, _ = opening(text)
    return [f'{path}:{line}: R3 {metric} {current[metric]} exceeds {limits[metric]}'
            for metric in ("opening_words", "opening_negatives") if current[metric] > limits[metric]]


def check_r4(path, text, current, recorded):
    limit = max(recorded["limit_sections"], 1) if recorded is not None else 1
    if current["limit_sections"] > limit:
        return [f'{path}:1: R4 limit_sections {current["limit_sections"]} exceeds {limit}']
    return []


def link_destination(value):
    """Read a Markdown destination, including escaped or balanced parentheses."""
    value = value.lstrip()
    if value.startswith("<"):
        return value[1:].split(">", 1)[0]
    depth = 0
    result = []
    escaped = False
    for char in value:
        if escaped:
            result.append(char)
            escaped = False
        elif char == "\\":
            escaped = True
        elif depth == 0 and (char == ")" or char.isspace()):
            break
        else:
            depth += (char == "(") - (char == ")")
            result.append(char)
    return "".join(result)


def check_r5(root, path, text):
    lines = [(n, re.sub(r"(`+).*?\1", "", line)) for n, line in visible_lines(text)]
    definitions = {}
    targets = set()
    for number, line in lines:
        if m := re.match(r"^ {0,3}\[([^\]]+)\]:\s*(.+)", line):
            target = link_destination(m[2])
            definitions[" ".join(m[1].casefold().split())] = target
            targets.add((number, target))
        for m in re.finditer(r"\]\(\s*", line):
            targets.add((number, link_destination(line[m.end():])))
        for m in re.finditer(r'''\b(?:href|src)\s*=\s*["']([^"']+)["']''', line, re.I):
            targets.add((number, html.unescape(m[1])))
    for number, line in lines:
        for m in re.finditer(r"\[([^\]]+)\](?:\[([^\]]*)\])?", line):
            label = " ".join((m[2] or m[1]).casefold().split())
            if label in definitions:
                targets.add((number, definitions[label]))
    errors = []
    for number, target in sorted(targets):
        url = urlsplit(target)
        if url.scheme or url.netloc or not url.path or url.path.startswith("/"):
            continue
        local = root / Path(path).parent / unquote(url.path)
        if not local.exists():
            errors.append(f"{path}:{number}: R5 relative link target does not exist: {target}")
    return errors


def check_document(root, path, text, recorded):
    errors = []
    if not Path(path).name.startswith("team-"):
        current = measure(text)
        for check in (check_r1, check_r2, check_r3, check_r4):
            errors.extend(check(path, text, current, recorded))
    return errors + check_r5(root, path, text)


def snapshot(root):
    return {path: measure((root / path).read_text(encoding="utf-8"))
            for path in document_paths(root) if not Path(path).name.startswith("team-")}


def read_baseline(root):
    data = json.loads((root / BASELINE).read_text(encoding="utf-8"))
    if not isinstance(data, dict) or data.get("version") != 1 or not isinstance(data.get("files"), dict):
        raise ValueError(f"Invalid baseline format: {BASELINE}")
    for path, values in data["files"].items():
        if not isinstance(values, dict) or set(values) != set(METRICS):
            raise ValueError(f"Invalid baseline metrics for {path}")
        for key, value in values.items():
            valid = type(value) in (int, float) if key == "density" else type(value) is int
            if not valid or not math.isfinite(value) or value < 0:
                raise ValueError(f"Invalid baseline {key} for {path}")
    return data["files"]


def main(argv=None, *, root=ROOT):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--write-baseline", action="store_true", help="Record the initial four metrics once")
    mode.add_argument("--update-baseline", action="store_true", help="Record equal or lower metrics after edits")
    mode.add_argument("--self-test", action="store_true", help="Run synthetic checks and rule mutation tests")
    args = parser.parse_args(argv)
    if args.self_test:
        return 0 if run_self_tests() else 1
    try:
        baseline_path = root / BASELINE
        if args.write_baseline and baseline_path.exists():
            raise ValueError(f"Baseline already exists: {BASELINE}; use --update-baseline")
        recorded = {} if args.write_baseline else read_baseline(root)
        current = snapshot(root)
        errors = []
        paths = document_paths(root)
        for path in paths:
            text = (root / path).read_text(encoding="utf-8")
            if args.write_baseline:
                errors.extend(check_r5(root, path, text))
            else:
                errors.extend(check_document(root, path, text, recorded.get(path)))
        if args.update_baseline:
            for path, values in current.items():
                if path in recorded:
                    errors.extend(f"{path}:1: baseline {key} {values[key]} exceeds {recorded[path][key]}"
                                  for key in METRICS if values[key] > recorded[path][key])
        if errors:
            print("\n".join(errors), file=sys.stderr)
            return 1
        if args.write_baseline or args.update_baseline:
            data = json.dumps({"version": 1, "files": dict(sorted((recorded | current).items()))},
                              indent=2, ensure_ascii=False) + "\n"
            baseline_path.parent.mkdir(parents=True, exist_ok=True)
            if args.write_baseline:
                with baseline_path.open("x", encoding="utf-8") as target:
                    target.write(data)
            else:
                with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=baseline_path.parent,
                                                 delete=False) as target:
                    temporary = Path(target.name)
                    target.write(data)
                try:
                    os.replace(temporary, baseline_path)
                finally:
                    temporary.unlink(missing_ok=True)
            print(f"Baseline saved: {len(current)} pages in {BASELINE}")
        print(f"Docs style OK: {len(paths)} pages checked (Team pages: R5 only).")
        return 0
    except (OSError, ValueError) as error:
        print(f"Docs style error: {error}", file=sys.stderr)
        return 1


class StyleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.path = "docs/dev/reference/sample.md"

    def write(self, text, path=None):
        target = self.root / (path or self.path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")

    def invoke(self, *args):
        output = io.StringIO()
        with redirect_stdout(output), redirect_stderr(output):
            code = main(list(args), root=self.root)
        return code, output.getvalue()

    def page(self, words=500, negatives=0):
        return ("# Page\n\nUse this page.\n\n## Details\n\n"
                + "word " * (words - 5 - negatives) + "not " * negatives + "\n")

    def baseline(self, text):
        self.write(text)
        self.assertEqual(self.invoke("--write-baseline")[0], 0)

    def assert_rule(self, rule, text, baseline=None, path=None):
        errors = check_document(self.root, path or self.path, text, baseline)
        self.assertTrue(any(f": {rule} " in error for error in errors), errors)
        self.assertTrue(all((path or self.path) in error for error in errors), errors)

    def test_ac1_repository_snapshot_passes_after_baseline(self):
        recorded = snapshot(ROOT)
        self.assertTrue(recorded)
        errors = []
        for path in document_paths(ROOT):
            errors.extend(check_document(ROOT, path, (ROOT / path).read_text(encoding="utf-8"),
                                         recorded.get(path)))
        self.assertEqual(errors, [])

    def test_ac1_existing_page_passes_after_baseline(self):
        self.baseline(self.page(negatives=12))
        self.assertEqual(self.invoke()[0], 0)
        data = json.loads((self.root / BASELINE).read_text())
        self.assertEqual(set(data["files"][self.path]),
                         {"density", "opening_words", "opening_negatives", "limit_sections"})

    def test_ac1_three_added_negative_sentences_fail_r1(self):
        self.baseline(self.page())
        self.write(self.page() + "It is not ready. It is not ready. It is not ready.\n")
        code, output = self.invoke()
        self.assertEqual(code, 1, output)
        self.assertIn(self.path + ":1: R1 ", output)

    def test_ac1_existing_long_negative_opening_is_recorded(self):
        text = "# Page\n\n" + "word " * 80 + "not ready.\n\n## Limits\n\nA.\n\n## Boundaries\n\nB.\n"
        self.baseline(text)
        self.assertEqual(self.invoke()[0], 0)
        recorded = json.loads((self.root / BASELINE).read_text())["files"][self.path]
        self.assertEqual(recorded["opening_words"], 82)
        self.assertEqual(recorded["opening_negatives"], 1)
        self.assertEqual(recorded["limit_sections"], 2)

    def test_ac1_longer_existing_opening_fails_r3(self):
        self.baseline("# Page\n\nUse this page.\n")
        self.write("# Page\n\nUse this helpful page.\n")
        code, output = self.invoke()
        self.assertEqual(code, 1, output)
        self.assertIn(self.path + ":3: R3 ", output)

    def test_ac1_added_opening_negation_fails_r3(self):
        self.baseline("# Page\n\nUse this page.\n")
        self.write("# Page\n\nUse not pages.\n")
        code, output = self.invoke()
        self.assertEqual(code, 1, output)
        self.assertIn(self.path + ":3: R3 ", output)

    def test_ac1_r1_allows_half_point_rounding_margin(self):
        text = self.page(negatives=3)
        old = measure(text)
        old["density"] -= 0.5
        self.assertEqual(check_document(self.root, self.path, text, old), [])
        old["density"] -= 0.01
        self.assert_rule("R1", text, old)

    def test_ac2_new_density_six_passes_r2(self):
        text = self.page(negatives=3)
        self.assertEqual(measure(text)["density"], 6.0)
        self.assertEqual(check_document(self.root, self.path, text, None), [])

    def test_ac2_new_high_density_fails_r2(self):
        self.assert_rule("R2", self.page(negatives=5))

    def test_ac2_new_eighty_word_opening_fails_r3(self):
        self.assert_rule("R3", "# Page\n\n" + "word " * 80 + "\n")

    def test_ac2_new_affirmative_sixty_word_opening_passes_r3(self):
        text = "# Page\n\n" + "word " * 60 + "\n"
        self.assertEqual(check_document(self.root, self.path, text, None), [])

    def test_ac2_new_negative_opening_fails_r3(self):
        self.assert_rule("R3", "# Page\n\nThis cannot work.\n")

    def test_ac2_html_opening_keeps_negative_terms(self):
        self.assert_rule("R3", "# Page\n\n<p>This cannot work.</p>\n\nUse this page.\n")

    def test_ac2_setext_headings_keep_opening_and_limits_checks(self):
        self.assert_rule("R3", "Page\n====\n\n" + "word " * 80 + "\n")
        self.assert_rule("R4", "# Page\n\nUse this page.\n\nLimits\n------\n\nA.\n\nBoundaries\n----------\n\nB.\n")

    def test_ac2_short_english_pages_skip_density_rules(self):
        text = "# Page\n\nUse this page.\n\n" + "not " * 250
        self.assertEqual(check_document(self.root, self.path, text, None), [])
        self.assertEqual(check_document(self.root, self.path, text, measure(self.page())), [])

    def test_ac2_one_limits_section_passes_r4(self):
        text = self.page() + "\n## Limits\n\nA short boundary.\n"
        self.assertEqual(check_document(self.root, self.path, text, None), [])

    def test_ac2_multiple_limits_sections_fail_r4(self):
        for heading in ("Known limits", "Boundaries", "限制", "边界"):
            with self.subTest(heading=heading):
                self.assert_rule("R4", self.page() + f"\n## Limits\n\nA.\n\n### {heading}\n\nB.\n")

    def test_ac1_additional_existing_limits_section_fails_r4(self):
        text = self.page() + "\n## Limits\n\nA.\n\n## Boundaries\n\nB.\n"
        self.assertEqual(check_document(self.root, self.path, text, measure(text)), [])
        self.assert_rule("R4", text + "\n## Known limits\n\nC.\n", measure(text))

    def test_ac1_r4_zero_baseline_allows_one_limits_section(self):
        text = self.page()
        self.baseline(text)
        before = (self.root / BASELINE).read_bytes()
        self.write(text + "\n## Limits\n\nA short boundary.\n")
        code, output = self.invoke()
        self.assertEqual(code, 0, output)
        self.assertEqual((self.root / BASELINE).read_bytes(), before)

    def test_ac1_r4_zero_baseline_rejects_two_limits_sections(self):
        text = self.page()
        self.baseline(text)
        self.write(text + "\n## Limits\n\nA.\n\n## Boundaries\n\nB.\n")
        code, output = self.invoke()
        self.assertEqual(code, 1, output)
        self.assertIn(self.path + ":1: R4 limit_sections 2 exceeds 1", output)

    def test_ac1_r4_two_baseline_allows_two_limits_sections(self):
        text = self.page() + "\n## Limits\n\nA.\n\n## Boundaries\n\nB.\n"
        self.baseline(text)
        code, output = self.invoke()
        self.assertEqual(code, 0, output)

    def test_ac1_r4_two_baseline_rejects_three_limits_sections(self):
        text = self.page() + "\n## Limits\n\nA.\n\n## Boundaries\n\nB.\n"
        self.baseline(text)
        self.write(text + "\n## Known limits\n\nC.\n")
        code, output = self.invoke()
        self.assertEqual(code, 1, output)
        self.assertIn(self.path + ":1: R4 limit_sections 3 exceeds 2", output)

    def test_ac2_chinese_density_counts_han_and_negative_terms(self):
        self.assertEqual(measure("正文" * 499 + "不未")["density"], 2.0)
        self.assertEqual(measure("无法禁止不得")["density"], 500.0)
        good = "# 页面\n\n使用本页。\n\n" + "文" * 992 + "不" * 6
        self.assertEqual(check_document(self.root, "README.zh-CN.md", good, None), [])
        bad = good + "不" * 10
        self.assert_rule("R2", bad, path="README.zh-CN.md")
        self.assert_rule("R1", bad, measure(good), path="README.zh-CN.md")
        self.assert_rule("R3", "# 页面\n\n无法使用。\n", path="README.zh-CN.md")

    def test_ac2_english_negative_terms_use_word_boundaries(self):
        text = "NOT never cannot can't won't notable cannotary nevermore"
        self.assertEqual(measure(text)["density"], 625.0)

    def test_ac2_team_pages_only_check_links(self):
        text = "# Page\n\n" + "not " * 400 + "\n\n## Limits\n\nA.\n\n## Boundaries\n\nB.\n"
        path = "docs/dev/reference/team-sample.md"
        self.assertEqual(check_document(self.root, path, text, None), [])
        self.assert_rule("R5", text + "\n[Missing](missing.md)\n", path=path)

    def test_ac3_existing_relative_links_pass_r5(self):
        self.write("# Target\n", "docs/dev/reference/target (one).md")
        text = ('# Page\n\nUse this page.\n\n[Link](target%20%28one%29.md#part)\n'
                '![Image](<target (one).md>)\n[Reference][target]\n'
                '[target]: <target (one).md> "Title"\n'
                '<a href="target%20%28one%29.md">Target</a>\n'
                '[External](https://example.com/missing) [Mail](mailto:a@example.com) [Here](#part)\n'
                '`[Literal](missing.md)`\n```md\n[Example](missing.md)\n```\n')
        self.assertEqual(check_document(self.root, self.path, text, None), [])

    def test_ac3_missing_relative_link_fails_r5_with_line(self):
        code, output = self.invoke("--write-baseline")
        self.assertEqual(code, 0, output)
        self.write("# Page\n\nUse this page.\n\n[Missing](missing.md)\n")
        code, output = self.invoke()
        self.assertEqual(code, 1, output)
        self.assertIn(self.path + ":5: R5 ", output)
        self.assertIn("missing.md", output)

    def test_ac3_missing_image_reference_and_html_fail_r5(self):
        for link in ('![Image](missing.png)', '[Link][bad]\n\n[bad]: missing.md',
                     '<img src="missing.png" />', '[Link](missing(1).md)'):
            with self.subTest(link=link):
                self.assert_rule("R5", "# Page\n\nUse this page.\n\n" + link)

    def test_ac4_update_accepts_lower_values(self):
        self.baseline(self.page(negatives=4))
        target = self.root / BASELINE
        data = json.loads(target.read_text())
        for key in data["files"][self.path]:
            data["files"][self.path][key] += 10
        target.write_text(json.dumps(data))
        code, output = self.invoke("--update-baseline")
        self.assertEqual(code, 0, output)
        self.assertEqual(json.loads(target.read_text())["files"][self.path],
                         measure((self.root / self.path).read_text()))

    def test_ac4_update_rejects_each_higher_value_without_writing(self):
        text = "# Page\n\nIt is not ready.\n\n## Limits\n\n" + "word " * 400
        self.baseline(text)
        target = self.root / BASELINE
        original = json.loads(target.read_text())
        for metric in original["files"][self.path]:
            with self.subTest(metric=metric):
                data = json.loads(json.dumps(original))
                data["files"][self.path][metric] -= 0.01 if metric == "density" else 1
                target.write_text(json.dumps(data))
                before = target.read_bytes()
                code, output = self.invoke("--update-baseline")
                self.assertEqual(code, 1, output)
                self.assertIn(metric, output)
                self.assertEqual(target.read_bytes(), before)

    def test_ac4_write_baseline_refuses_overwrite(self):
        self.baseline(self.page())
        before = (self.root / BASELINE).read_bytes()
        code, output = self.invoke("--write-baseline")
        self.assertEqual(code, 1, output)
        self.assertEqual((self.root / BASELINE).read_bytes(), before)

    def test_ac4_update_checks_new_pages_before_adding_them(self):
        self.baseline(self.page())
        self.write(self.page(negatives=5), "docs/series/new.md")
        before = (self.root / BASELINE).read_bytes()
        code, output = self.invoke("--update-baseline")
        self.assertEqual(code, 1, output)
        self.assertIn("R2", output)
        self.assertEqual((self.root / BASELINE).read_bytes(), before)
        self.write(self.page(negatives=3), "docs/series/new.md")
        self.assertEqual(self.invoke("--update-baseline")[0], 0)
        self.assertIn("docs/series/new.md", json.loads((self.root / BASELINE).read_text())["files"])

    def test_ac4_missing_or_invalid_baseline_fails(self):
        self.write(self.page())
        self.assertEqual(self.invoke()[0], 1)
        self.baseline(self.page())
        (self.root / BASELINE).write_text('{"version": 1, "files": {"x": {"density": NaN}}}')
        self.assertEqual(self.invoke()[0], 1)

    def test_style_guide_follows_its_reference_structure(self):
        path = "docs/dev/STYLE.md"
        text = (ROOT / path).read_text(encoding="utf-8")
        self.assertEqual(check_document(ROOT, path, text, None), [])
        headings = [line for _, line in visible_lines(text) if line.startswith("## ")]
        self.assertEqual(headings, ["## Use it", "## How it behaves", "## Limits", "## Code and tests"])


def run_rule_tests(stream):
    return unittest.TextTestRunner(stream=stream, verbosity=2).run(
        unittest.defaultTestLoader.loadTestsFromTestCase(StyleTests))


class SelfTestTests(unittest.TestCase):
    def test_ac5_rule_suite_passes(self):
        result = run_rule_tests(io.StringIO())
        self.assertTrue(result.wasSuccessful(), result.failures + result.errors)

    def test_ac5_disabling_any_rule_fails_self_test(self):
        examples = ("test_ac1_three_added_negative_sentences_fail_r1",
                    "test_ac2_new_high_density_fails_r2",
                    "test_ac2_new_eighty_word_opening_fails_r3",
                    "test_ac2_multiple_limits_sections_fail_r4",
                    "test_ac3_missing_relative_link_fails_r5_with_line")
        for rule in range(1, 6):
            with self.subTest(rule=f"R{rule}"):
                with patch.dict(globals(), {f"check_r{rule}": lambda *args: []}):
                    result = run_rule_tests(io.StringIO())
                self.assertFalse(result.wasSuccessful(), f"Self-test missed disabled R{rule}")
                self.assertTrue(any(examples[rule - 1] in str(test)
                                    for test, _ in result.failures + result.errors),
                                f"Self-test missed the R{rule} counterexample")


def run_self_tests():
    suite = unittest.TestSuite(unittest.defaultTestLoader.loadTestsFromTestCase(case)
                               for case in (StyleTests, SelfTestTests))
    return unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful()


if __name__ == "__main__":
    sys.exit(main())
