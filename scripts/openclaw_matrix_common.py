"""Shared paths and fixtures for OpenClaw security matrix tests (all platforms)."""

from __future__ import annotations

import os
import re
import sys
import tempfile
from pathlib import Path

DLP_CANARY = "SMR-MATRIX-DLP-CANARY-PORTABLE-FIXTURE"
DLP_SECRET_FILE = "matrix-secret.txt"
MARKER_FILE = "matrix-marker.txt"


def is_windows() -> bool:
    return os.name == "nt"


def smr_config_dir() -> Path:
    override = os.environ.get("SMR_CONFIG_DIR", "").strip()
    if override:
        return Path(override)
    if is_windows():
        appdata = os.environ.get("APPDATA", "").strip()
        if appdata:
            return Path(appdata) / "securemodelroute"
    if sys.platform == "darwin":
        return Path.home() / "Library/Application Support/securemodelroute"
    xdg = os.environ.get("XDG_CONFIG_HOME", "").strip()
    if xdg:
        return Path(xdg) / "securemodelroute"
    return Path.home() / ".config" / "securemodelroute"


def smr_traffic_dir() -> Path:
    override = os.environ.get("SMR_TRAFFIC_DIR", "").strip()
    if override:
        return Path(override)
    return smr_config_dir() / "traffic"


def _looks_like_windows_path(text: str) -> bool:
    return bool(re.match(r"^[A-Za-z]:", text.replace("\\", "/")))


def safe_matrix_root(root: Path | str | None = None) -> Path:
    if root is None:
        raw = os.environ.get("SMR_MATRIX_ROOT", "").strip()
        if raw:
            root = Path(raw.replace("\\", "/"))
        else:
            return matrix_root()
    text = str(root).replace("\\", "/")
    if not is_windows() and _looks_like_windows_path(text):
        return Path(tempfile.gettempdir()) / "smr-matrix"
    path = Path(text)
    if path.is_absolute():
        return path
    return path.resolve()


def matrix_root() -> Path:
    override = os.environ.get("SMR_MATRIX_ROOT", "").strip()
    if override:
        return safe_matrix_root(override)
    if is_windows():
        staging = os.environ.get("SMR_GUEST_STAGING", "").strip()
        if staging:
            return Path(staging.replace("\\", "/")) / "smr-matrix"
    return Path(tempfile.gettempdir()) / "smr-matrix"


def matrix_layout(root: Path | str | None = None) -> dict[str, str]:
    if root is None:
        base_text = str(safe_matrix_root(None).resolve()).replace("\\", "/")
    else:
        base_text = str(root).replace("\\", "/").rstrip("/")
        if not _looks_like_windows_path(base_text) and not Path(base_text).is_absolute():
            base_text = str(Path(base_text).resolve()).replace("\\", "/")
    return {
        "matrix_root": base_text,
        "dlp_dir": f"{base_text}/dlp-data",
        "dlp_secret": f"{base_text}/dlp-data/{DLP_SECRET_FILE}",
        "path_deny_access": f"{base_text}/protected-access",
        "path_deny_modify": f"{base_text}/protected-modify",
        "path_deny_delete": f"{base_text}/protected-delete",
        "path_open": f"{base_text}/open-area",
        "ops_tmp": f"{base_text}/ops-scratch",
    }


def ensure_fixtures(root: Path | None = None) -> dict[str, str]:
    paths = matrix_layout(root)
    for key in (
        "path_deny_access",
        "path_deny_modify",
        "path_deny_delete",
        "path_open",
        "ops_tmp",
    ):
        p = Path(paths[key])
        p.mkdir(parents=True, exist_ok=True)
        marker = p / MARKER_FILE
        if not marker.is_file():
            marker.write_text(f"matrix fixture: {key}\n", encoding="utf-8")

    dlp_dir = Path(paths["dlp_dir"])
    dlp_dir.mkdir(parents=True, exist_ok=True)
    secret = dlp_dir / DLP_SECRET_FILE
    if not secret.is_file():
        secret.write_text(
            f"{DLP_CANARY}\nPortable matrix fixture for file DLP indexing.\n",
            encoding="utf-8",
        )
    return paths


def write_env_file(out: Path, paths: dict[str, str], *, platform: str | None = None) -> None:
    plat = platform or ("windows" if is_windows() else "unix")
    lines = [
        f"SMR_MATRIX_PLATFORM={plat}",
        f"SMR_MATRIX_ROOT={paths['matrix_root']}",
        f"SMR_MATRIX_DLP_DIR={paths['dlp_dir']}",
        f"SMR_MATRIX_DLP_SECRET={paths['dlp_secret']}",
        f"SMR_MATRIX_PATH_DENY_ACCESS={paths['path_deny_access']}",
        f"SMR_MATRIX_PATH_DENY_MODIFY={paths['path_deny_modify']}",
        f"SMR_MATRIX_PATH_DENY_DELETE={paths['path_deny_delete']}",
        f"SMR_MATRIX_PATH_OPEN={paths['path_open']}",
        f"SMR_MATRIX_OPS_TMP={paths['ops_tmp']}",
        f"SMR_MATRIX_DLP_CANARY={DLP_CANARY}",
    ]
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
