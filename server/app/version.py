"""Single-source application version reader (reads repo-root `VERSION`)."""
from __future__ import annotations

from pathlib import Path

_VERSION_FILE = Path(__file__).resolve().parents[2] / "VERSION"


def get_version() -> str:
    try:
        v = _VERSION_FILE.read_text(encoding="utf-8").strip()
        if v:
            return v
    except OSError:
        pass
    return "0.1.0"


APP_VERSION = get_version()
