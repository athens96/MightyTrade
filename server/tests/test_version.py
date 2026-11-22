"""Tests for application semantic versioning synchronization (MT-11)."""
import json
import re
from pathlib import Path

from app.version import APP_VERSION

SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")


def test_version_files_and_health_endpoint_in_sync(client):
    root = Path(__file__).resolve().parents[2]
    root_ver = (root / "VERSION").read_text(encoding="utf-8").strip()
    pkg_ver = json.loads((root / "client" / "package.json").read_text(encoding="utf-8"))["version"]

    assert SEMVER_RE.match(root_ver), f"Invalid semver in VERSION: {root_ver}"
    assert root_ver == pkg_ver == APP_VERSION

    res = client.get("/api/health")
    assert res.status_code == 200
    assert res.json["status"] == "ok"
    assert res.json["version"] == root_ver
