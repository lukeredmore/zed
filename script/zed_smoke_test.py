#!/usr/bin/env python3
"""Smoke test for the packaged Zed.dmg artifact.

Mounts the DMG, verifies the Zed.app bundle structure and Info.plist metadata,
validates `zed --system-specs` version formatting (ensuring the auto-updater
does not enter an infinite update loop), verifies single-instance TCP handshake
isolation from vanilla Zed and delegation to modded Zed, and confirms the app
stays alive when launched with no arguments from $HOME.
"""

import os
import plistlib
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

BASE_PORT = 43737
MODDED_PORT_OFFSET = 400
STABLE_CHANNEL_OFFSET = 200


class FakeInstance:
    def __init__(self, port: int, handshake: bytes):
        self.port = port
        self.handshake = handshake
        self.received_payloads: list[str] = []
        self._stop = threading.Event()
        self._server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server.bind(("127.0.0.1", port))
        self._server.listen(4)
        self._server.settimeout(0.2)
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self):
        while not self._stop.is_set():
            try:
                conn, _ = self._server.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with conn:
                conn.sendall(self.handshake)
                chunks = []
                while True:
                    data = conn.recv(4096)
                    if not data:
                        break
                    chunks.append(data)
                if chunks:
                    self.received_payloads.append(
                        b"".join(chunks).decode("utf-8", errors="replace")
                    )

    def close(self):
        self._stop.set()
        try:
            self._server.close()
        except OSError:
            pass
        self._thread.join(timeout=1.0)


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, text=True, capture_output=True, **kwargs)


def verify_bundle_metadata(app_path: str, zed_bin: str):
    info_plist_path = os.path.join(app_path, "Contents", "Info.plist")
    assert os.path.isfile(info_plist_path), f"Missing Info.plist at {info_plist_path}"
    with open(info_plist_path, "rb") as f:
        plist = plistlib.load(f)

    assert plist.get("CFBundleExecutable") == "zed", plist.get("CFBundleExecutable")
    assert plist.get("CFBundleIdentifier") == "dev.zed.Zed", plist.get(
        "CFBundleIdentifier"
    )
    assert os.path.isfile(zed_bin) and os.access(zed_bin, os.X_OK), (
        f"zed binary not executable: {zed_bin}"
    )

    short_ver = plist.get("CFBundleShortVersionString")
    bundle_ver = plist.get("CFBundleVersion")
    commit_sha = plist.get("ZedCommitSHA")
    upstream_ver = plist.get("ZedUpstreamVersion")

    assert short_ver, "Missing CFBundleShortVersionString in Info.plist"
    assert "+" not in short_ver, (
        f"CFBundleShortVersionString must be clean upstream semver (got {short_ver!r})"
    )
    assert bundle_ver, "Missing CFBundleVersion in Info.plist"
    assert commit_sha and len(commit_sha) == 7, (
        f"ZedCommitSHA must be 7-char short SHA (got {commit_sha!r})"
    )
    assert upstream_ver == short_ver, (
        f"ZedUpstreamVersion ({upstream_ver!r}) != CFBundleShortVersionString ({short_ver!r})"
    )

    specs = run([zed_bin, "--system-specs"], timeout=30)
    assert specs.returncode == 0, (
        f"--system-specs failed ({specs.returncode}):\n{specs.stdout}\n{specs.stderr}"
    )
    expected_version = f"v{short_ver}+stable.{bundle_ver}.{commit_sha}"
    assert expected_version in specs.stdout, (
        f"Expected {expected_version!r} in --system-specs output:\n{specs.stdout}"
    )
    print(f"[PASS] --system-specs verified ({expected_version})")


def verify_single_instance_and_launch(zed_bin: str):
    uid = os.getuid()
    vanilla_port = BASE_PORT + STABLE_CHANNEL_OFFSET + uid
    modded_port = BASE_PORT + MODDED_PORT_OFFSET + STABLE_CHANNEL_OFFSET + uid

    with tempfile.TemporaryDirectory(prefix="zed-smoke-") as home:
        env = os.environ.copy()
        env["HOME"] = home
        os.makedirs(os.path.join(home, "Library", "Logs", "Zed"), exist_ok=True)
        os.makedirs(
            os.path.join(home, "Library", "Application Support", "Zed"), exist_ok=True
        )

        # 1. Verify modded Zed does NOT connect to a running vanilla Zed instance.
        vanilla = FakeInstance(vanilla_port, b"Zed Editor Stable Instance Running\n")
        modded = FakeInstance(
            modded_port, b"Zed Modded Editor Stable Instance Running\n"
        )
        try:
            proc = run(
                [zed_bin, os.path.join(home, "test.md")],
                env=env,
                timeout=30,
            )
            assert proc.returncode == 0, (
                f"Expected exit 0 when delegating to existing modded instance, got {proc.returncode}:\n"
                f"stdout={proc.stdout}\nstderr={proc.stderr}"
            )
            assert not vanilla.received_payloads, (
                f"Modded Zed unexpectedly contacted vanilla Zed port {vanilla_port}: {vanilla.received_payloads}"
            )
            assert len(modded.received_payloads) == 1, (
                f"Expected modded instance on port {modded_port} to receive 1 open payload, got {modded.received_payloads}"
            )
            print(
                f"[PASS] Single-instance isolation (vanilla port {vanilla_port} untouched, modded port {modded_port} received payload)"
            )
        finally:
            modded.close()
            vanilla.close()

        # 2. Verify launching with no arguments from $HOME stays alive without panicking.
        proc = subprocess.Popen(
            [zed_bin],
            cwd=home,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            time.sleep(1.5)
            poll = proc.poll()
            if poll is not None:
                out, err = proc.communicate(timeout=5)
                raise AssertionError(
                    f"Zed exited prematurely with code {poll} when launched from $HOME:\nstdout={out}\nstderr={err}"
                )
            print("[PASS] Zed stayed alive for 1.5s when launched with no args from $HOME")
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


def main():
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <path/to/Zed.dmg>", file=sys.stderr)
        sys.exit(2)

    dmg_path = os.path.abspath(sys.argv[1])
    assert os.path.isfile(dmg_path), f"DMG not found: {dmg_path}"

    mount_dir = tempfile.mkdtemp(prefix="zed-dmg-mount-")
    app_copy_dir = tempfile.mkdtemp(prefix="zed-app-copy-")
    attached = False
    try:
        attach = run(
            [
                "hdiutil",
                "attach",
                "-nobrowse",
                "-readonly",
                "-mountpoint",
                mount_dir,
                dmg_path,
            ],
            timeout=60,
        )
        assert attach.returncode == 0, f"hdiutil attach failed:\n{attach.stderr}"
        attached = True

        mounted_app = os.path.join(mount_dir, "Zed.app")
        assert os.path.isdir(mounted_app), f"Zed.app not found in {mount_dir}"

        local_app = os.path.join(app_copy_dir, "Zed.app")
        shutil.copytree(mounted_app, local_app, symlinks=True)
        zed_bin = os.path.join(local_app, "Contents", "MacOS", "zed")

        verify_bundle_metadata(local_app, zed_bin)
        verify_single_instance_and_launch(zed_bin)
        print("[PASS] All Zed.dmg smoke tests passed.")
    finally:
        if attached:
            run(["hdiutil", "detach", "-force", mount_dir], timeout=30)
        shutil.rmtree(mount_dir, ignore_errors=True)
        shutil.rmtree(app_copy_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
