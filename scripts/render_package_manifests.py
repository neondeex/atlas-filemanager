#!/usr/bin/env python3
"""Render the winget, AUR and Flatpak manifests for a release.

The checked-in manifests describe the previous release; this rewrites the
version, the release date and every SHA-256 from the files actually published.

    render_package_manifests.py --version 0.1.6 --dist dist --output target/package-manifests
"""
import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from linux_release import release_notes, sha256, validate_version  # noqa: E402

WINGET = ROOT / "packaging/windows/winget/manifests/n/NeonDeex/AtlasFM"
AUR = ROOT / "packaging/linux/aur"
FLATPAK = ROOT / "packaging/linux/flatpak"
HASH = re.compile(r"[0-9a-fA-F]{64}")


def previous_version(text):
    found = re.search(r"download/v([^/\s']+)/", text)
    if not found:
        raise SystemExit("Manifest has no release download URL")
    return found[1]


def render(text, version, digest, date=None, upper=False, old=None):
    old = old or previous_version(text)
    text = text.replace(old, version).replace(old.replace("-", "_"), version.replace("-", "_"))
    text = HASH.sub(digest.upper() if upper else digest, text)
    if date:
        text = re.sub(r"(?m)^ReleaseDate: .*$", f"ReleaseDate: {date}", text)
    return text


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = validate_version(args.version)
    date, _ = release_notes((ROOT / "CHANGELOG.md").read_text(encoding="utf-8"), version)
    tarball = sha256(args.dist / f"atlas-{version}-linux-x86_64.tar.gz")
    installer = sha256(args.dist / f"AtlasSetup-{version}.exe")

    jobs = [(path, tarball, False) for path in (AUR / "PKGBUILD", AUR / ".SRCINFO", *FLATPAK.glob("*.yaml"))]
    source_dir = sorted(WINGET.iterdir())[-1]
    jobs += [(path, installer, True) for path in source_dir.glob("*.yaml")]
    for path, digest, upper in jobs:
        relative = path.relative_to(ROOT / "packaging")
        old = source_dir.name if upper else None
        if upper:
            relative = relative.parent.with_name(version) / path.name
        target = args.output / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        text = path.read_text(encoding="utf-8")
        target.write_text(render(text, version, digest, date if upper else None, upper, old), encoding="utf-8", newline="\n")
        print(target)


if __name__ == "__main__":
    main()
