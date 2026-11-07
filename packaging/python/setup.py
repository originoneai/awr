"""Build from the staged native payload produced by scripts/release/build_packages.py."""
import json
from pathlib import Path

from setuptools import Distribution, setup
from setuptools.command.bdist_wheel import bdist_wheel

root = Path(__file__).parent
metadata = json.loads((root / "awr_binary" / "_build.json").read_text())


class BinaryDistribution(Distribution):
    def has_ext_modules(self):
        return True


class BinaryWheel(bdist_wheel):
    def get_tag(self):
        return "py3", "none", metadata["wheel_platform"]


setup(
    name="originoneai-awr",
    version=metadata["python_version"],
    description="AWR — the project delivery platform for people and AI",
    long_description=(root / "README.md").read_text(),
    long_description_content_type="text/markdown",
    license="Apache-2.0",
    license_files=["LICENSE", "THIRD_PARTY_LICENSES.txt"],
    url="https://github.com/originoneai/awr",
    python_requires=">=3.9",
    packages=["awr_binary"],
    package_data={"awr_binary": ["bin/*", "_build.json"]},
    entry_points={"console_scripts": [
        "awr=awr_binary.cli:awr", "awr-mcp=awr_binary.cli:awr_mcp",
    ]},
    distclass=BinaryDistribution,
    cmdclass={"bdist_wheel": BinaryWheel},
)
