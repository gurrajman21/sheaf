"""Fetch public real-world .xlsx test files into corpus/public/<source>/.

Sources are the regression-test folders of Apache POI and LibreOffice: hundreds
of files saved by real Excel versions, many of them bug reproductions. Each repo
is sparse-cloned at a pinned commit (recorded in corpus/public/SOURCES.lock on
first fetch) so runs are reproducible.

Note: LibreOffice's own test files are biased toward things LibreOffice already
handles; reports break results down per source for that reason.

Usage: python fetch_public.py [--refresh]
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PUBLIC = ROOT / "corpus" / "public"
SRC = PUBLIC / ".src"
LOCK = PUBLIC / "SOURCES.lock"
MAX_BYTES = 10 * 1024 * 1024

SOURCES = {
    "poi": ("https://github.com/apache/poi.git", "test-data/spreadsheet"),
    "libreoffice": ("https://github.com/LibreOffice/core.git", "sc/qa/unit/data/xlsx"),
}


def git(*args, cwd=None):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def fetch(name: str, url: str, subdir: str, sha: str | None) -> str:
    repo = SRC / name
    if not repo.exists():
        repo.mkdir(parents=True)
        git("init", "-q", cwd=repo)
        git("remote", "add", "origin", url, cwd=repo)
        git("sparse-checkout", "set", "--no-cone", f"/{subdir}/", cwd=repo)
    git("fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", sha or "HEAD", cwd=repo)
    git("checkout", "-q", "FETCH_HEAD", cwd=repo)
    return git("rev-parse", "HEAD", cwd=repo)


def is_ooxml(path: Path) -> bool:
    try:
        with zipfile.ZipFile(path) as z:
            return "[Content_Types].xml" in z.namelist()
    except (zipfile.BadZipFile, OSError):
        return False


def main():
    refresh = "--refresh" in sys.argv
    lock = json.loads(LOCK.read_text()) if LOCK.exists() and not refresh else {}
    for name, (url, subdir) in SOURCES.items():
        print(f"[{name}] fetching {url} {subdir} @ {lock.get(name, 'HEAD')}")
        sha = fetch(name, url, subdir, lock.get(name))
        lock[name] = sha
        dest = PUBLIC / name
        shutil.rmtree(dest, ignore_errors=True)
        dest.mkdir(parents=True)
        kept = skipped = 0
        for f in sorted((SRC / name / subdir).rglob("*")):
            if f.suffix.lower() not in (".xlsx", ".xlsm") or not f.is_file():
                continue
            # Encrypted/corrupt files aren't zips; they test password handling, not fidelity.
            if f.stat().st_size > MAX_BYTES or not is_ooxml(f):
                skipped += 1
                continue
            shutil.copy2(f, dest / f.name)
            kept += 1
        print(f"[{name}] {sha[:12]}: kept {kept}, skipped {skipped} (non-zip or >10MB)")
    LOCK.write_text(json.dumps(lock, indent=2) + "\n")


if __name__ == "__main__":
    main()
