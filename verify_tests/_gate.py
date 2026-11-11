"""Shared plumbing for the Drive Sync release gate (not a test case itself).

unit cases:    run one JUnit test from the repo's own suite through Gradle and confirm it ran.
android cases: drive the *installed release APK* over adb; never reinstall or sign anything.
"""
from __future__ import annotations

import glob
import os
import shutil
import subprocess
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = "com.almodhayan.drivesync"


# ----------------------------------------------------------------------------- unit

def _java_home() -> str | None:
    if os.environ.get("DRIVESYNC_JAVA_HOME"):
        return os.environ["DRIVESYNC_JAVA_HOME"]
    found = sorted(glob.glob(str(Path.home() / ".jdk" / "jdk-21*")))
    return found[-1] if found else os.environ.get("JAVA_HOME")


def gradle_unit(fqcn: str, method: str, timeout: int = 900) -> None:
    """Runs exactly one JUnit method (fresh, never UP-TO-DATE) and asserts it executed and passed."""
    gradlew = ROOT / ("gradlew.bat" if os.name == "nt" else "gradlew")
    env = dict(os.environ)
    if _java_home():
        env["JAVA_HOME"] = _java_home()
    proc = subprocess.run(
        [str(gradlew), ":app:testDebugUnitTest", "--tests", f"{fqcn}.{method}", "--rerun", "--console=plain", "-q"],
        cwd=ROOT, env=env, capture_output=True, text=True, timeout=timeout,
    )
    report = ROOT / "app/build/test-results/testDebugUnitTest" / f"TEST-{fqcn}.xml"
    assert report.is_file(), f"Gradle produced no JUnit report for {fqcn}\n{proc.stdout[-3000:]}{proc.stderr[-3000:]}"
    cases = [c for c in ET.parse(report).getroot().iter("testcase") if c.get("name") == method]
    assert cases, f"{fqcn}.{method} did not execute (renamed or filtered out?)"
    problems = [child.get("message") or child.tag for c in cases for child in c if child.tag in ("failure", "error")]
    assert not problems, f"{fqcn}.{method} failed: {problems}"
    assert proc.returncode == 0, f"Gradle exited {proc.returncode}\n{proc.stdout[-3000:]}{proc.stderr[-3000:]}"


# ----------------------------------------------------------------------------- android

def _adb_path() -> str:
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if sdk:
        cand = Path(sdk) / "platform-tools" / ("adb.exe" if os.name == "nt" else "adb")
        if cand.is_file():
            return str(cand)
    return shutil.which("adb") or "adb"


def _serial() -> str:
    if os.environ.get("DRIVESYNC_SERIAL"):
        return os.environ["DRIVESYNC_SERIAL"]
    out = subprocess.run([_adb_path(), "devices"], capture_output=True, text=True).stdout
    ready = [l.split()[0] for l in out.splitlines()[1:] if l.strip().endswith("\tdevice")]
    assert len(ready) == 1, f"need exactly one authorized device (set DRIVESYNC_SERIAL); adb sees: {out.strip()}"
    return ready[0]


def adb(*args: str, timeout: int = 60) -> subprocess.CompletedProcess:
    return subprocess.run([_adb_path(), "-s", _serial(), *args], capture_output=True, text=True, timeout=timeout)


def require_release_app() -> None:
    """The gate targets the installed release build; it never installs one itself."""
    out = adb("shell", "pm", "list", "packages", "--user", "0", PACKAGE).stdout
    assert f"package:{PACKAGE}" in out.split(), f"{PACKAGE} is not installed on the device"


def sh(cmd: str, timeout: int = 60) -> str:
    """One shell line on the device (a single argument, so the device shell does the parsing)."""
    return adb("shell", cmd, timeout=timeout).stdout


# ----------------------------------------------------------------------------- UI, by testTag only
# Compose exposes Modifier.testTag as the resource-id (testTagsAsResourceId). Screen text is never
# matched; the only text read is data the test created itself (folder names) and numbers in counts.

import re
import time
import xml.etree.ElementTree as ET  # parses UiAutomator's dump of this device, a trusted local source
from dataclasses import dataclass


@dataclass
class Node:
    rid: str
    text: str
    desc: str
    x1: int
    y1: int
    x2: int
    y2: int
    enabled: bool

    @property
    def center(self) -> tuple[int, int]:
        return (self.x1 + self.x2) // 2, (self.y1 + self.y2) // 2

    def contains(self, other: "Node") -> bool:
        return self.x1 <= other.x1 and self.y1 <= other.y1 and other.x2 <= self.x2 and other.y2 <= self.y2


def dump() -> list[Node]:
    for _ in range(4):   # "could not get idle state" while something animates: try again
        if "dumped" in sh("uiautomator dump /sdcard/vt-ui.xml"):
            xml = sh("cat /sdcard/vt-ui.xml")
            if xml.strip().startswith("<?xml"):
                break
        time.sleep(1)
    else:
        raise AssertionError("uiautomator could not dump the screen")
    nodes = []
    for e in ET.fromstring(xml).iter("node"):
        x1, y1, x2, y2 = map(int, re.findall(r"\d+", e.get("bounds", "[0,0][0,0]")))
        nodes.append(Node(e.get("resource-id", ""), e.get("text", ""), e.get("content-desc", ""), x1, y1, x2, y2, e.get("enabled") == "true"))
    return nodes


def find(tag: str, nodes: list[Node] | None = None) -> Node | None:
    return next((n for n in (nodes if nodes is not None else dump()) if n.rid == tag), None)


def wait_for(tag: str, timeout: float = 20, enabled: bool = False) -> Node:
    end = time.time() + timeout
    while time.time() < end:
        n = find(tag)
        if n and (n.enabled or not enabled):
            return n
        time.sleep(1)
    raise AssertionError(f"'{tag}' did not appear within {timeout:.0f}s")


def tap(n: Node) -> None:
    sh(f"input tap {n.center[0]} {n.center[1]}")


def tap_tag(tag: str, timeout: float = 20) -> None:
    tap(wait_for(tag, timeout, enabled=True))


def swipe_up() -> None:
    sh("input swipe 540 1700 540 700 300")


def swipe_down() -> None:
    sh("input swipe 540 700 540 1700 300")


def scroll_to(tag: str, max_swipes: int = 8) -> Node:
    for _ in range(max_swipes + 1):
        n = find(tag)
        if n:
            return n
        swipe_up()
        time.sleep(0.8)
    raise AssertionError(f"'{tag}' not found after scrolling")


def reveal(tag: str, section: str) -> Node:
    """A control inside a collapsed section: scroll for it, else open its section header and look again."""
    try:
        return scroll_to(tag, max_swipes=6)
    except AssertionError:
        for _ in range(6):   # back to the top so the header is found on the way down
            swipe_down()
        tap(scroll_to(section))
        time.sleep(1)
        return scroll_to(tag)


def scroll_to_text(text: str, max_swipes: int = 8) -> Node:
    """For data the test created itself (a folder name), never for UI copy."""
    for _ in range(max_swipes + 1):
        n = next((x for x in dump() if x.text == text), None)
        if n:
            return n
        swipe_up()
        time.sleep(0.8)
    raise AssertionError(f"'{text}' not found after scrolling")


def type_text(s: str) -> None:
    assert re.fullmatch(r"[A-Za-z0-9._-]+", s), "keep typed fixture names to safe characters"
    sh(f"input text {s}")


def first_int(s: str) -> int | None:
    m = re.search(r"\d[\d,]*", s)
    return int(m.group(0).replace(",", "")) if m else None


# ----------------------------------------------------------------------------- app flows

def launch(route: str | None = None, cold: bool = False) -> None:
    if cold:
        sh(f"am force-stop {PACKAGE}")
        time.sleep(1)
    extra = f" --es route {route}" if route else ""
    sh(f"am start -n {PACKAGE}/.ui.MainActivity{extra}")
    time.sleep(3)


def go_home() -> None:
    launch()
    for _ in range(6):
        if find("home_sync_all"):
            return
        sh("input keyevent KEYCODE_BACK")
        time.sleep(1)
        launch()
    wait_for("home_sync_all", 10)


def run_id() -> str:
    return time.strftime("%m%d%H%M%S")


def make_local_folder(name: str, files: dict[str, str]) -> str:
    path = f"/sdcard/{name}"
    sh(f"rm -rf {path}; mkdir -p {path}")
    for fname, body in files.items():
        sh(f"printf '%s' '{body}' > {path}/{fname}")
    return path


def local_listing(path: str) -> dict[str, str]:
    """name -> md5 of every file directly in [path]."""
    out = sh(f"cd {path} && md5sum * 2>/dev/null")
    return {line.split(None, 1)[1].strip(): line.split()[0] for line in out.splitlines() if line.strip()}


def create_pair_to_preview(name: str) -> None:
    """New pair from /sdcard/<name> to a new My Drive/<name>, ending on the first-sync preview."""
    go_home()
    nodes = dump()
    tap(find("home_new_pair", nodes) or find("home_empty_action", nodes) or wait_for("home_new_pair"))
    tap_tag("path_phone")
    time.sleep(2)
    tap(scroll_to_text(name))
    time.sleep(1.5)
    tap_tag("picker_use")
    tap_tag("path_drive")
    tap_tag("remote_new_folder", timeout=60)
    tap_tag("new_folder_name")
    type_text(name)
    tap_tag("new_folder_create")
    tap_tag("picker_use", timeout=60)
    tap_tag("editor_preview")
    wait_for("summary_upload", timeout=90)


def first_sync(name: str, files: dict[str, str]) -> str:
    path = make_local_folder(name, files)
    create_pair_to_preview(name)
    tap_tag("dryrun_start")
    tap_tag("run_done", timeout=180)
    return path


def open_pair(name: str) -> None:
    go_home()
    tap(scroll_to(f"pair_card:{name}"))
    wait_for("pair_edit", 15)


def count_in(tag: str) -> int:
    """The number shown inside a tagged tile: its child whose whole text is a number."""
    nodes = dump()
    tile = find(tag, nodes) or wait_for(tag)
    for n in nodes:
        if tile.contains(n) and re.fullmatch(r"\d[\d,]*", n.text):
            return int(n.text.replace(",", ""))
    raise AssertionError(f"no number inside '{tag}'")


def side_count(side: str) -> int:
    """Files on one side as the pair screen shows them; an empty side shows no number."""
    n = wait_for(f"path_{side}_meta", 15)
    return first_int(n.text) or 0


def history_has(trigger: str) -> bool:
    for _ in range(4):
        if find(f"run_row:{trigger}"):
            return True
        swipe_up()
        time.sleep(0.8)
    return False


def remove_pair(name: str) -> None:
    go_home()
    card = find(f"pair_card:{name}")
    if card is None:
        return
    tap(card)
    tap_tag("pair_edit")
    tap(reveal("editor_remove", "editor_advanced"))
    time.sleep(2)


def cleanup(name: str) -> None:
    try:
        remove_pair(name)
    finally:
        sh(f"rm -rf /sdcard/{name}")


def automation(on: bool) -> str | None:
    """Settings › Automation: set Allow automation and, when on, return the secret shown there."""
    go_home()
    tap_tag("nav_settings")
    tap(scroll_to("settings_automation"))
    toggle = wait_for("automation_toggle")
    if (find("automation_secret") is not None) != on:
        tap(toggle)
        time.sleep(1.5)
    secret = None
    if on:
        secret = wait_for("automation_secret", 10).text.strip()
        assert re.fullmatch(r"[0-9a-f]{32}", secret), f"unexpected secret format: {secret!r}"
    else:
        time.sleep(1)
        assert find("automation_secret") is None, "automation still shows a secret after turning it off"
    sh("input keyevent KEYCODE_BACK")
    return secret


def broadcast(action: str, secret: str | None = None) -> str:
    extra = f" --es secret {secret}" if secret else ""
    return sh(f"am broadcast -a com.almodhayan.drivesync.{action}{extra} -n {PACKAGE}/.scheduler.SyncIntentReceiver")
