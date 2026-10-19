"""Fetch public real-world .docx test files into corpus/docx/public/<source>/.

Same approach as fetch_public.py (sparse clones pinned in corpus/docx/SOURCES.lock):
regression-test folders of Apache POI, LibreOffice Writer and python-docx. Many are
bug reproductions saved by real Word versions; those carry Word's own layout hints
(docProps/app.xml <Pages> and <w:lastRenderedPageBreak/>), which docx_truth.py uses
as ground truth.

LibreOffice's own test files favor LibreOffice; reports break results down per source.

Usage: python fetch_public_docx.py [--refresh]
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / "corpus" / "docx"
PUBLIC = BASE / "public"
SRC = BASE / ".src"
LOCK = BASE / "SOURCES.lock"
MAX_BYTES = 10 * 1024 * 1024

# name -> (repo, [subdirs])
SOURCES = {
    "poi": ("https://github.com/apache/poi.git", ["test-data/document"]),
    "libreoffice": ("https://github.com/LibreOffice/core.git", [
        "sw/qa/extras/ooxmlexport/data",
        "sw/qa/extras/ooxmlimport/data",
        "sw/qa/extras/layout/data",
    ]),
    "python-docx": ("https://github.com/python-openxml/python-docx.git", ["features/steps/test_files", "tests/test_files"]),
}


def git(*args, cwd=None):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def fetch(name: str, url: str, subdirs: list[str], sha: str | None) -> str:
    repo = SRC / name
    if not repo.exists():
        repo.mkdir(parents=True)
        git("init", "-q", cwd=repo)
        git("remote", "add", "origin", url, cwd=repo)
        git("sparse-checkout", "set", "--no-cone", *[f"/{d}/" for d in subdirs], cwd=repo)
    git("fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", sha or "HEAD", cwd=repo)
    git("checkout", "-q", "FETCH_HEAD", cwd=repo)
    return git("rev-parse", "HEAD", cwd=repo)


def is_docx(path: Path) -> bool:
    try:
        with zipfile.ZipFile(path) as z:
            return "word/document.xml" in z.namelist()
    except (zipfile.BadZipFile, OSError):
        return False


def main():
    refresh = "--refresh" in sys.argv
    lock = json.loads(LOCK.read_text()) if LOCK.exists() and not refresh else {}
    for name, (url, subdirs) in SOURCES.items():
        print(f"[{name}] fetching {url} @ {lock.get(name, 'HEAD')}", flush=True)
        sha = fetch(name, url, subdirs, lock.get(name))
        lock[name] = sha
        dest = PUBLIC / name
        shutil.rmtree(dest, ignore_errors=True)
        dest.mkdir(parents=True)
        kept = skipped = 0
        seen: set[str] = set()
        for sub in subdirs:
            for f in sorted((SRC / name / sub).rglob("*")):
                if f.suffix.lower() not in (".docx", ".docm") or not f.is_file():
                    continue
                if f.stat().st_size > MAX_BYTES or not is_docx(f):
                    skipped += 1
                    continue
                out = f.name if f.name not in seen else f"{f.parent.name}__{f.name}"
                seen.add(out)
                shutil.copy2(f, dest / out)
                kept += 1
        print(f"[{name}] {sha[:12]}: kept {kept}, skipped {skipped} (non-docx or >10MB)", flush=True)
    LOCK.write_text(json.dumps(lock, indent=2) + "\n")


if __name__ == "__main__":
    main()
