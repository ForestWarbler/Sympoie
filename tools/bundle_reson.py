#!/usr/bin/env python3
"""Builds the Reson plugin and bundles it as Reson.vst3 and Reson.clap in target/bundled.

    python3 tools/bundle_reson.py [--debug] [--universal] [--install]

--debug      bundles a debug build instead of a release build.
--universal  on macOS, builds for Apple Silicon and Intel Macs into one bundle (needs the
             aarch64-apple-darwin and x86_64-apple-darwin Rust targets).
--install    copies the bundles into this user's plug-in folders, replacing older copies.
"""
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NAME = "Reson"
PACKAGE = "reson-plugin"
LIBRARY = "reson"
IDENTIFIER = "com.sympoie.reson"
OUTPUT = ROOT / "target" / "bundled"


def version():
    text = (ROOT / "Cargo.toml").read_text()
    section = text.split("[workspace.package]", 1)[1]
    return re.search(r'^version\s*=\s*"([^"]+)"', section, re.M).group(1)


def build(profile, target=None):
    command = ["cargo", "build", "-p", PACKAGE]
    if profile == "release":
        command.append("--release")
    if target:
        command += ["--target", target]
    subprocess.run(command, cwd=ROOT, check=True)
    folder = ROOT / "target" / (target or "") / profile
    system = platform.system()
    if system == "Darwin":
        return folder / f"lib{LIBRARY}.dylib"
    if system == "Windows":
        return folder / f"{LIBRARY}.dll"
    return folder / f"lib{LIBRARY}.so"


def mac_metadata(bundle):
    contents = bundle / "Contents"
    (contents / "PkgInfo").write_text("BNDL????")
    (contents / "Info.plist").write_text(f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>English</string>
    <key>CFBundleExecutable</key>
    <string>{NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>{IDENTIFIER}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>{NAME}</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleShortVersionString</key>
    <string>{version()}</string>
    <key>CFBundleVersion</key>
    <string>{version()}</string>
    <key>NSHumanReadableCopyright</key>
    <string>GPL-3.0-or-later</string>
    <key>NSHighResolutionCapable</key>
    <true/>
  </dict>
</plist>
""")


def bundle(library):
    """Lays the library out as a VST3 and a CLAP bundle; returns their paths."""
    shutil.rmtree(OUTPUT / f"{NAME}.vst3", ignore_errors=True)
    shutil.rmtree(OUTPUT / f"{NAME}.clap", ignore_errors=True)
    if (OUTPUT / f"{NAME}.clap").is_file():
        (OUTPUT / f"{NAME}.clap").unlink()
    system = platform.system()
    machine = "x86_64" if platform.machine().lower() in ("x86_64", "amd64") else "aarch64"
    vst3 = OUTPUT / f"{NAME}.vst3"
    clap = OUTPUT / f"{NAME}.clap"
    if system == "Darwin":
        for home in (vst3, clap):
            binary = home / "Contents" / "MacOS" / NAME
            binary.parent.mkdir(parents=True)
            shutil.copy2(library, binary)
            mac_metadata(home)
            # Apple Silicon refuses unsigned code; a local signature is enough to load.
            subprocess.run(["codesign", "--force", "--sign", "-", str(home)], check=True)
    elif system == "Windows":
        binary = vst3 / "Contents" / f"{machine}-win" / f"{NAME}.vst3"
        binary.parent.mkdir(parents=True)
        shutil.copy2(library, binary)
        shutil.copy2(library, clap)
    else:
        binary = vst3 / "Contents" / f"{machine}-linux" / f"{NAME}.so"
        binary.parent.mkdir(parents=True)
        shutil.copy2(library, binary)
        shutil.copy2(library, clap)
    return vst3, clap


def install(vst3, clap):
    system = platform.system()
    home = Path.home()
    if system == "Darwin":
        folders = (home / "Library/Audio/Plug-Ins/VST3", home / "Library/Audio/Plug-Ins/CLAP")
    elif system == "Windows":
        common = Path(os.environ.get("COMMONPROGRAMFILES", r"C:\Program Files\Common Files"))
        folders = (common / "VST3", common / "CLAP")
    else:
        folders = (home / ".vst3", home / ".clap")
    for source, folder in zip((vst3, clap), folders):
        folder.mkdir(parents=True, exist_ok=True)
        destination = folder / source.name
        if destination.is_dir():
            shutil.rmtree(destination)
        elif destination.exists():
            destination.unlink()
        if source.is_dir():
            shutil.copytree(source, destination)
        else:
            shutil.copy2(source, destination)
        print(f"Installed {destination}")


def main():
    arguments = set(sys.argv[1:])
    unknown = arguments - {"--debug", "--universal", "--install"}
    if unknown:
        sys.exit(f"unknown option {sorted(unknown)[0]}; see the top of this file")
    profile = "debug" if "--debug" in arguments else "release"
    OUTPUT.mkdir(parents=True, exist_ok=True)
    if "--universal" in arguments:
        if platform.system() != "Darwin":
            sys.exit("--universal is for macOS")
        parts = [build(profile, target) for target in ("aarch64-apple-darwin", "x86_64-apple-darwin")]
        library = OUTPUT / f"lib{LIBRARY}-universal.dylib"
        subprocess.run(["lipo", "-create", "-output", str(library), *map(str, parts)], check=True)
    else:
        library = build(profile)
    vst3, clap = bundle(library)
    print(f"Created {vst3}\nCreated {clap}")
    if "--install" in arguments:
        install(vst3, clap)


if __name__ == "__main__":
    main()
