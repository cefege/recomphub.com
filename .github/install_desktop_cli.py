import hashlib
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile

VERSION = "0.7.10"
# Digests published on the official DioxusLabs/dioxus v0.7.10 release assets.
DIGESTS = {
    "aarch64-apple-darwin": "4aff37aea6b8fbce7bee3f8bbbea1bd71f60e0fa09537f35759f6494da861e37",
    "x86_64-pc-windows-msvc": "45eb4f87b7f86fdba8508ee7e0ef1f9e13cf8f2ffcc6a2d5858d0e8ea7dd1531",
}
target = sys.argv[1]
expected_digest = DIGESTS[target]
binary_name = "dx.exe" if target.endswith("windows-msvc") else "dx"
install_dir = Path(os.environ["RUNNER_TEMP"]) / f"dioxus-{VERSION}-{target}"
install_dir.mkdir(parents=True, exist_ok=True)
url = f"https://github.com/DioxusLabs/dioxus/releases/download/v{VERSION}/dx-{target}.zip"
with tempfile.TemporaryFile() as archive_file:
    with urllib.request.urlopen(url, timeout=90) as response:
        shutil.copyfileobj(response, archive_file)
    archive_file.seek(0)
    if hashlib.file_digest(archive_file, "sha256").hexdigest() != expected_digest:
        raise RuntimeError("Official Dioxus CLI archive digest does not match the pinned release")
    archive_file.seek(0)
    with zipfile.ZipFile(archive_file) as archive:
        members = [entry for entry in archive.infolist() if not entry.is_dir() and PurePosixPath(entry.filename).name == binary_name]
        if len(members) != 1:
            raise RuntimeError("Official Dioxus CLI archive has an unexpected executable layout")
        executable = install_dir / binary_name
        with archive.open(members[0]) as source, executable.open("wb") as destination:
            shutil.copyfileobj(source, destination)
executable.chmod(0o755)
subprocess.run([str(executable), "--version"], check=True)
with open(os.environ["GITHUB_PATH"], "a", encoding="utf-8") as github_path:
    github_path.write(f"{install_dir}\n")
