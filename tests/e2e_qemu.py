#!/usr/bin/env python3
"""Run the x86_64-musl release binary under QEMU against local mock APIs."""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BINARY = ROOT / "target/x86_64-unknown-linux-musl/release/po0-auto"
OLD_IP = "8.8.4.4"
NEW_IP = "1.1.1.1"
PRIMARY_TOKEN = "e2e-primary-token"
FLAKY_TOKEN = "e2e-flaky-token"
TELEGRAM_TOKEN = "e2e-telegram-token"


class TestState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.path_calls: Counter[str] = Counter()
        self.firewall_calls: Counter[str] = Counter()
        self.telegram_messages: list[dict[str, str]] = []


class MockHandler(BaseHTTPRequestHandler):
    server: "MockServer"

    def do_GET(self) -> None:  # noqa: N802 - required by BaseHTTPRequestHandler
        parsed = urlparse(self.path)
        with self.server.test_state.lock:
            self.server.test_state.path_calls[parsed.path] += 1
            path_call = self.server.test_state.path_calls[parsed.path]

        if parsed.path == "/bad-ip":
            self.respond(200, b"not an ip\n")
            return
        if parsed.path == "/public-ip":
            # Keep the first address for two polls so the retry path completes,
            # then emulate a real public IPv4 change.
            value = OLD_IP if path_call <= 2 else NEW_IP
            self.respond(200, f"{value}\n".encode())
            return
        if parsed.path == "/firewall":
            query = parse_qs(parsed.query)
            if query.get("action") != ["add"] or len(query.get("token", [])) != 1:
                self.respond(400, b"invalid query")
                return
            token = query["token"][0]
            with self.server.test_state.lock:
                self.server.test_state.firewall_calls[token] += 1
                token_call = self.server.test_state.firewall_calls[token]
            if token == FLAKY_TOKEN and token_call == 1:
                self.respond(500, b"temporary failure")
            elif token in (PRIMARY_TOKEN, FLAKY_TOKEN):
                self.respond(204, b"")
            else:
                self.respond(403, b"unknown token")
            return
        self.respond(404, b"not found")

    def do_POST(self) -> None:  # noqa: N802 - required by BaseHTTPRequestHandler
        if self.path != f"/bot{TELEGRAM_TOKEN}/sendMessage":
            self.respond(404, b"not found")
            return
        length = int(self.headers.get("Content-Length", "0"))
        message = json.loads(self.rfile.read(length))
        with self.server.test_state.lock:
            self.server.test_state.path_calls[self.path] += 1
            self.server.test_state.telegram_messages.append(message)
        self.respond(200, b'{"ok":true}')

    def respond(self, status: int, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


class MockServer(ThreadingHTTPServer):
    def __init__(self, address: tuple[str, int], test_state: TestState) -> None:
        super().__init__(address, MockHandler)
        self.test_state = test_state


def make_config(base_url: str, directory: Path) -> Path:
    config = directory / "config.yaml"
    config.write_text(
        f"""\
ip_sources:
  - name: invalid-first
    url: {base_url}/bad-ip
    response: plain_text
    timeout_seconds: 2
  - name: working-fallback
    url: {base_url}/public-ip
    response: plain_text
    timeout_seconds: 2

firewall:
  base_url: {base_url}/firewall
  timeout_seconds: 2

servers:
  - name: primary
    token: {PRIMARY_TOKEN}
  - name: flaky
    token: {FLAKY_TOKEN}

telegram:
  enabled: true
  api_base: {base_url}
  bot_token: {TELEGRAM_TOKEN}
  chat_id: "-100-test"
  timeout_seconds: 2

detection:
  fast_interval_seconds: 1
  slow_interval_seconds: 2
  slow_until_percent: 75
  history_sample_size: 10
  retention_days: 7
  retry_initial_seconds: 1
  retry_max_seconds: 2

storage:
  state_file: {directory}/state.json
  history_file: {directory}/history.json

user_agent: po0-auto-e2e/1
""",
        encoding="utf-8",
    )
    os.chmod(config, 0o600)
    return config


def wait_for_two_notifications(
    process: subprocess.Popen[str], test_state: TestState, timeout: float = 15.0
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"po0-auto exited early with status {process.returncode}")
        with test_state.lock:
            if len(test_state.telegram_messages) >= 2:
                return
        time.sleep(0.05)
    raise AssertionError("timed out waiting for initial and changed-IP notifications")


def main() -> int:
    binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_BINARY
    qemu = shutil.which("qemu-x86_64")
    if not qemu:
        raise SystemExit("qemu-x86_64 is not installed")
    if not binary.is_file():
        raise SystemExit(f"x86_64 binary not found: {binary}")

    test_state = TestState()
    server = MockServer(("127.0.0.1", 0), test_state)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    base_url = f"http://127.0.0.1:{server.server_port}"

    process: subprocess.Popen[str] | None = None
    logs = ""
    try:
        with tempfile.TemporaryDirectory(prefix="po0-auto-e2e-") as temp:
            directory = Path(temp)
            config = make_config(base_url, directory)
            environment = os.environ.copy()
            environment["RUST_LOG"] = "info"
            process = subprocess.Popen(
                [qemu, str(binary), "--config", str(config)],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                env=environment,
            )
            wait_for_two_notifications(process, test_state)
            process.send_signal(signal.SIGTERM)
            try:
                logs, _ = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                logs, _ = process.communicate(timeout=2)
                raise AssertionError("po0-auto did not stop after SIGTERM")
            if process.returncode != 0:
                raise AssertionError(f"po0-auto exited with status {process.returncode}\n{logs}")

            state = json.loads((directory / "state.json").read_text(encoding="utf-8"))
            history = json.loads((directory / "history.json").read_text(encoding="utf-8"))
            with test_state.lock:
                path_calls = test_state.path_calls.copy()
                firewall_calls = test_state.firewall_calls.copy()
                messages = list(test_state.telegram_messages)

            assert path_calls["/bad-ip"] >= 3, path_calls
            assert path_calls["/public-ip"] >= 3, path_calls
            assert firewall_calls[PRIMARY_TOKEN] == 2, firewall_calls
            assert firewall_calls[FLAKY_TOKEN] == 3, firewall_calls
            assert len(messages) == 2, messages
            assert OLD_IP in messages[0]["text"], messages[0]
            assert NEW_IP in messages[1]["text"], messages[1]
            assert all(message["chat_id"] == "-100-test" for message in messages)

            assert state["current_ipv4"] == NEW_IP, state
            assert state["synced_servers"] == {"flaky": NEW_IP, "primary": NEW_IP}, state
            assert state["pending_sync"] is None, state
            assert state["pending_notification"] is None, state
            assert len(history["records"]) == 1, history
            assert history["records"][0]["old_ipv4"] == OLD_IP, history
            assert history["records"][0]["new_ipv4"] == NEW_IP, history
            assert history["records"][0]["interval_seconds"] >= 1, history
            assert (directory / "state.json").stat().st_mode & 0o777 == 0o600
            assert (directory / "history.json").stat().st_mode & 0o777 == 0o600
            for secret in (PRIMARY_TOKEN, FLAKY_TOKEN, TELEGRAM_TOKEN):
                assert secret not in logs, "a credential appeared in process logs"

            print("PASS: x86_64 binary executed under QEMU")
            print("PASS: invalid IPv4 source fell back to the next source")
            print("PASS: failed server retried without repeating successful server")
            print("PASS: initial and changed IPv4 notifications were delivered")
            print("PASS: state/history persistence and 0600 permissions verified")
            print("PASS: SIGTERM shutdown and credential-safe logs verified")
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=2)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
