#!/usr/bin/env python3
"""Smoke test for the packaged Zed.dmg artifact.

Mounts the DMG, verifies the Zed.app bundle structure and Info.plist metadata,
validates `zed --system-specs` version formatting (ensuring the auto-updater
does not enter an infinite update loop), and verifies the macOS single-instance
TCP handshake:
- Case 1: a vanilla Zed holds its port. Modded Zed must start, bind its own
  modded port, and stay alive when launched from $HOME.
- Case 2: a modded Zed holds the modded port. A second modded Zed must print
  "zed is already running" and exit.
"""

import os
import plistlib
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

BASE_PORT = 43737
USER_BLOCK = 100
STABLE_OFFSET = 2 * USER_BLOCK
MODDED_PORT_OFFSET = 4 * USER_BLOCK
VANILLA_HANDSHAKE = b"Zed Editor Stable Instance Running"
MODDED_HANDSHAKE = b"Zed Modded Editor Stable Instance Running"
ALREADY_RUNNING = "zed is already running"


def user_port(port: int) -> int:
    max_uid = 0xFFFF - port
    return port + (os.getuid() % max_uid)


VANILLA_PORT = user_port(BASE_PORT + STABLE_OFFSET)
MODDED_PORT = user_port(BASE_PORT + STABLE_OFFSET + MODDED_PORT_OFFSET)


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, text=True, capture_output=True, **kwargs)


def read_handshake(port: int, length: int, timeout: float = 0.5) -> bytes | None:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=timeout) as conn:
            conn.settimeout(timeout)
            data = b""
            while len(data) < length:
                chunk = conn.recv(length - len(data))
                if not chunk:
                    break
                data += chunk
            return data
    except OSError:
        return None


class FakeInstance:
    """Answers every connection on `port` with `handshake`, like a running Zed."""

    def __init__(self, port: int, handshake: bytes):
        self.port = port
        self.handshake = handshake
        self.server: socket.socket | None = None

    def __enter__(self):
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            server.bind(("127.0.0.1", self.port))
        except OSError:
            server.close()
            if read_handshake(self.port, len(self.handshake)) == self.handshake:
                return self
            raise RuntimeError(
                f"port {self.port} is held by something that is not a matching Zed"
            )
        server.listen()
        self.server = server
        threading.Thread(target=self._serve, daemon=True).start()
        return self

    def _serve(self):
        assert self.server is not None
        while True:
            try:
                conn, _ = self.server.accept()
            except OSError:
                return
            with conn:
                try:
                    conn.sendall(self.handshake)
                except OSError:
                    pass

    def __exit__(self, *exc):
        if self.server:
            self.server.close()


def launch(binary: str, home: str) -> subprocess.Popen:
    env = dict(os.environ, HOME=home, RUST_LOG="info")
    env.pop("ZED_STATELESS", None)
    return subprocess.Popen(
        [binary],
        cwd=home,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )


def stop(proc: subprocess.Popen) -> str:
    if proc.poll() is None:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    output, _ = proc.communicate(timeout=10)
    return output or ""


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
    print(f"[PASS] --system-specs verified ({expected_version})", flush=True)


def case_runs_next_to_vanilla_zed(binary: str, home: str) -> str | None:
    with FakeInstance(VANILLA_PORT, VANILLA_HANDSHAKE):
        proc = launch(binary, home)
        handshake = None
        deadline = time.time() + 20
        while time.time() < deadline and proc.poll() is None:
            handshake = read_handshake(MODDED_PORT, len(MODDED_HANDSHAKE))
            if handshake == MODDED_HANDSHAKE:
                break
            time.sleep(0.02)
        if handshake == MODDED_HANDSHAKE:
            # Verify Zed stays alive past settings::init / GPUI startup
            # when launched outside a source checkout (cwd=home).
            time.sleep(1.5)
        early_exit = proc.poll()
        output = stop(proc)
    if ALREADY_RUNNING in output:
        return f"Modded Zed handed its launch to vanilla Zed on port {VANILLA_PORT}:\n{output}"
    if handshake != MODDED_HANDSHAKE:
        return f"Modded Zed did not answer on port {MODDED_PORT} (got {handshake!r}):\n{output}"
    if early_exit is not None or "panicked at" in output:
        return (
            f"Modded Zed crashed during startup after binding port {MODDED_PORT} "
            f"(exit={early_exit}):\n{output}"
        )
    return None


def case_hands_off_to_running_modded_zed(binary: str, home: str) -> str | None:
    with FakeInstance(MODDED_PORT, MODDED_HANDSHAKE):
        proc = launch(binary, home)
        try:
            proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            pass
        output = stop(proc)
    if ALREADY_RUNNING not in output:
        return f"A second modded Zed did not hand off to the running one on port {MODDED_PORT}:\n{output}"
    return None


def main() -> int:
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <path/to/Zed.dmg>", file=sys.stderr)
        return 2

    dmg_path = os.path.abspath(sys.argv[1])
    assert os.path.isfile(dmg_path), f"DMG not found: {dmg_path}"

    mount_dir = tempfile.mkdtemp(prefix="zed-dmg-mount-")
    app_copy_dir = tempfile.mkdtemp(prefix="zed-app-copy-")
    attached = False
    failures: list[str] = []
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

        for case in (
            case_runs_next_to_vanilla_zed,
            case_hands_off_to_running_modded_zed,
        ):
            with tempfile.TemporaryDirectory(prefix="zed-smoke-home-") as home:
                os.makedirs(os.path.join(home, "Library", "Logs", "Zed"), exist_ok=True)
                os.makedirs(
                    os.path.join(home, "Library", "Application Support", "Zed"),
                    exist_ok=True,
                )
                error = case(zed_bin, home)
            print(f"[{'FAIL' if error else 'PASS'}] {case.__name__}", flush=True)
            if error:
                failures.append(error)
    finally:
        if attached:
            run(["hdiutil", "detach", "-force", mount_dir], timeout=30)
        shutil.rmtree(mount_dir, ignore_errors=True)
        shutil.rmtree(app_copy_dir, ignore_errors=True)

    for error in failures:
        print(error, file=sys.stderr)
    if not failures:
        print("[PASS] All Zed.dmg smoke tests passed.", flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
