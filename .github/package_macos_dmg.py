"""Package a built Apple Silicon app without indexing the temporary disk image."""

import plistlib
import subprocess
import sys
import tempfile
from pathlib import Path


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def main() -> None:
    app = Path(sys.argv[1]).resolve(strict=True)
    output = Path(sys.argv[2]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    image = output / f"{app.stem}_{info['CFBundleShortVersionString']}_aarch64.dmg"
    with tempfile.TemporaryDirectory(prefix="recomphub-dmg-") as temporary:
        root = Path(temporary)
        staging = root / "contents"
        staging.mkdir()
        run("/usr/bin/ditto", str(app), str(staging / app.name))
        (staging / "Applications").symlink_to("/Applications", target_is_directory=True)
        run(
            "/usr/bin/hdiutil", "create", "-volname", app.stem,
            "-srcfolder", str(staging), "-fs", "APFS", "-nospotlight",
            "-format", "UDZO", "-ov", str(image),
        )
        run("/usr/bin/hdiutil", "verify", str(image))
        mount = root / "mounted"
        mount.mkdir()
        run("/usr/bin/hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", str(mount), str(image))
        try:
            installed = mount / app.name
            run("/usr/bin/codesign", "--verify", "--deep", "--strict", str(installed))
            executable = installed / "Contents/MacOS" / info["CFBundleExecutable"]
            architecture = subprocess.check_output(["/usr/bin/lipo", "-archs", str(executable)], text=True).strip()
            if architecture != "arm64":
                raise RuntimeError(f"Expected only arm64 in the packaged app, got {architecture!r}")
        finally:
            run("/usr/bin/hdiutil", "detach", str(mount))
    print(f"Verified Apple Silicon disk image: {image}")


if __name__ == "__main__":
    main()
