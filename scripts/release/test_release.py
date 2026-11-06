#!/usr/bin/env python3
"""Release-channel and artifact-integrity checks without registry writes."""
from contextlib import redirect_stdout
import hashlib
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import assemble_release
from build_packages import ROOT, copy_contract_docs, python_version
from mcp_catalog import DOMAIN_TOOLS, validate_stdio_tools
import publish_npm


class ReleaseChecks(unittest.TestCase):
    def test_installed_catalog_requires_continuity_tools_and_unique_names(self):
        names = sorted(DOMAIN_TOOLS)
        self.assertEqual(validate_stdio_tools(list(reversed(names))), names)
        for invalid in [names[:-1], names + [names[0]],
                        names[:-1] + ["unrelated_tool"], [f"tool-{i}" for i in range(20)],
                        names + ["awr_projects_list"]]:
            with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                validate_stdio_tools(invalid)

    def test_packaged_docs_survive_repository_relocation(self):
        with tempfile.TemporaryDirectory() as tmp:
            package = Path(tmp)
            copy_contract_docs(package)
            installed = sorted(p.relative_to(package).as_posix() for p in package.rglob("*.md"))
            self.assertEqual(installed, [
                "docs/integrations/session-workflow.md",
                "docs/reference/cli-mcp-contract.md",
                "docs/reference/daily-work.md",
                "docs/reference/local-project-hub.md",
            ])
            for rel in installed:
                self.assertEqual((package / rel).read_bytes(),
                                 (ROOT / "docs/dev" / Path(rel).relative_to("docs")).read_bytes())

    def test_stable_and_prerelease_version_mapping(self):
        for original, expected in {
            "0.2.0": "0.2.0", "0.3.0-dev": "0.3.0.dev0",
            "0.3.0-alpha.2": "0.3.0a2", "0.3.0-beta.3": "0.3.0b3", "0.3.0-rc.1": "0.3.0rc1",
        }.items():
            self.assertEqual(python_version(original), expected)
        for invalid in ["latest", "0.2", "0.2.0+dirty", "0.2.0-preview"]:
            with self.assertRaises(ValueError):
                python_version(invalid)

    def package_set(self, root, version):
        npm = root / "npm"
        npm.mkdir()
        artifacts = {}
        tag = "next" if "-" in version else "latest"
        for suffix in ["", "-darwin-arm64", "-darwin-x64", "-linux-x64-gnu", "-linux-arm64-gnu", "-win32-x64"]:
            path = npm / ("package" + suffix + ".tgz")
            metadata = json.dumps({"name": "@originoneai/agent-work-runtime" + suffix,
                                   "version": version, "publishConfig": {"tag": tag}}).encode()
            with tarfile.open(path, "w:gz") as archive:
                info = tarfile.TarInfo("package/package.json")
                info.size = len(metadata)
                archive.addfile(info, io.BytesIO(metadata))
            artifacts[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
        manifest = {"version": version, "channel": tag, "platforms": sorted(assemble_release.PLATFORMS), "artifacts": artifacts}
        (root / "release-manifest.json").write_text(json.dumps(manifest))
        return manifest

    def test_publish_stable_latest_and_prerelease_next_with_wrapper_last(self):
        for version, tag in [("0.3.1", "latest"), ("0.3.1-rc.1", "next")]:
            with self.subTest(version=version), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                self.package_set(root, version)
                with patch.object(sys, "argv", ["publish_npm.py", str(root)]), patch("publish_npm.subprocess.run") as run, redirect_stdout(io.StringIO()):
                    publish_npm.main()
                commands = [call.args[0] for call in run.call_args_list]
                self.assertEqual(len(commands), 6)
                self.assertTrue(all(c[c.index("--tag")+1] == tag for c in commands))
                self.assertEqual(Path(commands[-1][2]).name, "package.tgz")
                self.assertIn("package-darwin-x64.tgz", [Path(c[2]).name for c in commands[:-1]])

    def test_wrong_channel_and_tampered_archive_never_publish(self):
        for fault in ["channel", "artifact", "missing_intel", "missing_intel_platform"]:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                manifest = self.package_set(root, "0.2.0")
                if fault == "channel":
                    manifest["channel"] = "next"
                    (root / "release-manifest.json").write_text(json.dumps(manifest))
                elif fault == "artifact":
                    with (root / "npm/package.tgz").open("ab") as stream:
                        stream.write(b"changed")
                else:
                    (root / "npm/package-darwin-x64.tgz").unlink()
                    if fault == "missing_intel_platform":
                        manifest["platforms"].remove("darwin-x64")
                        (root / "release-manifest.json").write_text(json.dumps(manifest))
                with patch.object(sys, "argv", ["publish_npm.py", str(root)]), patch("publish_npm.subprocess.run") as run:
                    with self.assertRaises(AssertionError):
                        publish_npm.main()
                    run.assert_not_called()

    def assembled_inputs(self, root):
        source = "1" * 40
        for platform in sorted(assemble_release.PLATFORMS):
            base = root / platform
            (base / "publish").mkdir(parents=True)
            artifacts = {}
            for name, content in [(platform+".whl", platform.encode()), (platform+".tgz", platform.encode()), ("wrapper.tgz", b"same-wrapper")]:
                (base / "publish" / name).write_bytes(content)
                artifacts[name] = hashlib.sha256(content).hexdigest()
            manifest = {"platform": platform, "version": "0.2.0", "python_version": "0.2.0", "source_sha": source,
                        "source_tree_clean": True, "artifacts": artifacts, "local_project_hub": True}
            (base / "manifest.json").write_text(json.dumps(manifest))
            checks = {"version_and_help": True, "exit_code_and_stderr": True,
                      "unicode_space_project_init_and_status": True, "task_context_and_intake": True,
                      "mcp_stdio_tools": sorted(DOMAIN_TOOLS), "local_project_hub": True}
            receipt = {"source_sha": source, "platform": platform, "artifact_sha256": artifacts, "npm": checks, "python": checks}
            (base / "installation-checks.json").write_text(json.dumps(receipt))
        return source

    def test_assembly_requires_matching_installation_and_source_receipts(self):
        for fault in [None, "source", "installation", "bytes", "missing_intel", "intel_installation", "missing_tool", "duplicate_tool", "wrong_tool", "missing_hub_receipt", "inconsistent_hub"]:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                source = self.assembled_inputs(root / "inputs")
                platform = root / "inputs/darwin-arm64"
                if fault == "missing_intel":
                    shutil.rmtree(root / "inputs/darwin-x64")
                elif fault == "intel_installation":
                    path = root / "inputs/darwin-x64/installation-checks.json"
                    data = json.loads(path.read_text())
                    data["npm"]["version_and_help"] = False
                    path.write_text(json.dumps(data))
                elif fault == "bytes":
                    (platform / "publish/darwin-arm64.whl").write_bytes(b"different")
                elif fault:
                    path = platform / ("manifest.json" if fault in ("source", "inconsistent_hub") else "installation-checks.json")
                    data = json.loads(path.read_text())
                    if fault == "source":
                        data["source_sha"] = "2" * 40
                    elif fault == "inconsistent_hub":
                        data["local_project_hub"] = False
                    elif fault == "missing_hub_receipt":
                        del data["python"]["local_project_hub"]
                    elif fault == "missing_tool":
                        data["python"]["mcp_stdio_tools"].remove("awr_session")
                    elif fault == "duplicate_tool":
                        data["python"]["mcp_stdio_tools"].append("awr_session")
                    elif fault == "wrong_tool":
                        data["python"]["mcp_stdio_tools"][0] = "unrelated_tool"
                    else:
                        data["python"]["task_context_and_intake"] = False
                    path.write_text(json.dumps(data))
                with patch.object(sys, "argv", ["assemble_release.py", str(root/"inputs"), "--output", str(root/"release"), "--expected-sha", source]), redirect_stdout(io.StringIO()):
                    if fault:
                        with self.assertRaises(AssertionError):
                            assemble_release.main()
                        self.assertFalse((root / "release").exists())
                    else:
                        assemble_release.main()
                        result = json.loads((root / "release/release-manifest.json").read_text())
                        self.assertEqual((result["version"], result["python_version"], result["channel"]), ("0.2.0", "0.2.0", "latest"))
                        self.assertEqual(len(result["artifacts"]), 11)
                        self.assertIn("darwin-x64", result["platforms"])

    def test_legacy_release_receipts_remain_usable_without_a_hub_declaration(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = self.assembled_inputs(root / "inputs")
            for platform in assemble_release.PLATFORMS:
                base = root / "inputs" / platform
                manifest = json.loads((base / "manifest.json").read_text())
                del manifest["local_project_hub"]
                (base / "manifest.json").write_text(json.dumps(manifest))
                receipt = json.loads((base / "installation-checks.json").read_text())
                for ecosystem in ("npm", "python"):
                    del receipt[ecosystem]["local_project_hub"]
                (base / "installation-checks.json").write_text(json.dumps(receipt))
            with patch.object(sys, "argv", ["assemble_release.py", str(root / "inputs"), "--output", str(root / "release"), "--expected-sha", source]), redirect_stdout(io.StringIO()):
                assemble_release.main()


if __name__ == "__main__":
    unittest.main()
