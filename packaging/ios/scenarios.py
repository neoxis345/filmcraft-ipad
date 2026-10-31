#!/usr/bin/env python3
"""Scenarios for FilmCraft running in the iPad simulator (start it with packaging/ios/sim.sh).

Drives the app through its control channel (127.0.0.1:9876, Debug builds only) and the simulator
(screenshots, the app's Documents folder). Every scenario raises on failure; the run prints one line
per scenario and exits non-zero when any failed.

    packaging/ios/scenarios.py                 every scenario once
    packaging/ios/scenarios.py --loop 10       ten rounds (relaunching the app between rounds)
    packaging/ios/scenarios.py menus import    only these
"""

import argparse
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
OUT = ROOT / "target" / "ios-sim"
SHOTS = OUT / "shots"
FIXTURES = ROOT / "target" / "fixtures"


def bundle():
    """The bundle identifier of the app sim.sh built (each developer signs with their own)."""
    plist = OUT / "DerivedData/Build/Products/Debug-iphonesimulator/FilmCraft.app/Info.plist"
    out = subprocess.run(["/usr/libexec/PlistBuddy", "-c", "Print CFBundleIdentifier", str(plist)], capture_output=True, text=True, check=True)
    return out.stdout.strip()


class Control:
    def __init__(self, port=9876):
        self.token = (OUT / "token").read_text().strip()
        self.port = port
        self.sock = None
        self.n = 0

    def connect(self):
        for _ in range(90):
            try:
                self.sock = socket.create_connection(("127.0.0.1", self.port), timeout=120)
                self.f = self.sock.makefile("rwb")
                return
            except OSError:
                time.sleep(1)
        raise RuntimeError("the control channel does not answer")

    def call(self, method, params=None):
        if self.sock is None:
            self.connect()
        self.n += 1
        req = {"id": self.n, "token": self.token, "method": method}
        if params is not None:
            req["params"] = params
        self.f.write(json.dumps(req).encode() + b"\n")
        self.f.flush()
        line = self.f.readline()
        if not line:
            self.sock = None
            raise RuntimeError(f"{method}: connection closed")
        r = json.loads(line)
        if not r.get("ok", False):
            raise RuntimeError(f"{method}: {r.get('error')}")
        return r.get("result")

    def run(self, command, params=None):
        t = time.time()
        try:
            return self.call("engine.execute", {"command": command, "params": params or {}})
        finally:
            if os.environ.get("FC_TRACE"):
                print(f"    {command} {time.time() - t:.2f} s", flush=True)

    def elements(self, prefix=""):
        r = self.call("ui.elements", {"prefix": prefix} if prefix else {})
        return r.get("elements", r) if isinstance(r, dict) else r


def sim():
    return (OUT / "sim").read_text().strip()


def shot(name):
    SHOTS.mkdir(parents=True, exist_ok=True)
    tag = os.environ.get("FC_TAG", "")
    path = SHOTS / f"{tag + '-' if tag else ''}{name}.png"
    subprocess.run(["xcrun", "simctl", "io", sim(), "screenshot", str(path)], capture_output=True, check=False)
    return path


def documents():
    out = subprocess.run(["xcrun", "simctl", "get_app_container", sim(), bundle(), "data"], capture_output=True, text=True, check=True)
    return pathlib.Path(out.stdout.strip()) / "Documents"


def wait(what, check, timeout):
    """Poll `check` until it returns a true value; return (value, seconds)."""
    start = time.time()
    while time.time() - start < timeout:
        v = check()
        if v:
            return v, time.time() - start
        time.sleep(0.25)
    raise RuntimeError(f"{what}: not done after {timeout} s")


def ids(c, prefix=""):
    return {e.get("id") for e in c.elements(prefix) if isinstance(e, dict)}


def escape(c):
    # a recovery prompt (the scenarios end the app without saving) is answered first
    if c.call("ui.inspect").get("dialog") == "Recovery":
        c.call("ui.click", {"id": "recovery.discard"})
        time.sleep(0.3)
    for _ in range(3):
        c.call("ui.key", {"key": "Escape"})
        time.sleep(0.15)


# ------------------------------------------------------------------------------------- scenarios


def s_menus(c):
    """The File / Edit / … menus are on screen (folded into "Menus" when narrow) and open."""
    escape(c)
    found = ids(c, "menu.")
    if "menu.compact" in found:
        c.call("ui.click", {"id": "menu.compact"})
        time.sleep(0.4)
        found = ids(c, "menu.")
    for top in ["File", "Edit", "Clip", "Sequence", "Markers", "Window", "Help"]:
        if f"menu.{top}" not in found:
            raise RuntimeError(f"menu {top} missing: {sorted(found)}")
    before = len(c.elements())
    c.call("ui.click", {"id": "menu.File"})
    time.sleep(0.4)
    opened = len(c.elements())
    shot("menus-file")
    escape(c)
    if opened <= before:
        raise RuntimeError("the File menu did not open")
    return f"{len(found)} menu titles, File menu shows {opened - before} entries"


def s_import(c):
    """iPhone files import readable: HEVC 4K, Dolby Vision HEVC (what an iPhone records by default)
    and H.264 1080p."""
    names = ["iphone_hevc_4k.mov", "iphone_dvh1.mov", "h264_1080p.mp4"]
    missing = [n for n in names if not FIXTURES.joinpath(n).exists()]
    if missing:
        raise RuntimeError(f"missing fixtures {missing}: run packaging/ios/fixtures.sh")
    media = documents() / "Media"
    media.mkdir(parents=True, exist_ok=True)
    stamp = int(time.time() * 1000)
    paths = []
    for name in names:
        dst = media / f"{stamp}-{name}"
        shutil.copyfile(FIXTURES / name, dst)
        paths.append(str(dst))
    t0 = time.time()
    r = c.run("file.import", {"paths": paths})
    if r.get("errors"):
        raise RuntimeError(f"import errors: {r['errors']}")
    items = r.get("items", [])
    if len(items) != len(names):
        raise RuntimeError(f"{len(items)} items for {len(names)} files")

    def statuses():
        st = {x["item"]: x["status"] for x in c.run("media.status") if x.get("item") in items}
        return st if len(st) == len(items) else None

    st, _ = wait("media status", statuses, 30)
    bad = {n: st[i] for n, i in zip(names, items) if st[i] in ("unreadable", "missing", "offline")}
    if bad:
        raise RuntimeError(f"not readable: {bad}")
    took = time.time() - t0
    shot("import")
    return f"{len(items)} files readable in {took:.2f} s ({sorted(set(st.values()))})"


def s_playback(c):
    """The demo sequence plays: pictures reach the Program monitor and the app keeps answering.
    The drop rate is reported, not judged: a simulator decodes in software on a Mac that may be
    busy (a new simulator indexes for minutes), so it says nothing of an iPad's smoothness."""
    escape(c)
    before = c.call("perf.stats").get("monitor", {}).get("frame")
    c.call("ui.playback", {"action": "play"})
    time.sleep(6)
    st = c.call("perf.stats")
    c.call("ui.playback", {"action": "stop"})
    pb = st.get("playback", {})
    after = st.get("monitor", {}).get("frame")
    shot("playback")
    if not pb.get("shown") or after == before:
        raise RuntimeError(f"nothing played: {pb}")
    load = os.getloadavg()[0]
    return f"{pb.get('shown')} images, drop rate {pb.get('dropRate', 0):.0%} (Mac load {load:.0f}), ui fps {st.get('ui', {}).get('fps', 0):.0f}"


def s_edit(c):
    """Add edit, undo, redo."""
    c.run("sequence.addEdit", {"seconds": 3})
    c.run("edit.undo")
    c.run("edit.redo")
    c.run("edit.undo")
    return "ok"


def s_save(c):
    """Save the project into Documents/Projects and find it there."""
    path = documents() / "Projects" / "Scenario.fcproj"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    c.run("file.saveAs", {"path": str(path)})
    wait("project file", lambda: path.exists() and path.stat().st_size > 0, 20)
    return f"{path.stat().st_size} bytes"


TICKS = 254_016_000_000  # filmcraft_time::Tick per second


def clips(c, kind="video", track=0):
    seq = c.run("sequence.inspect")
    tracks = seq.get(kind, [])
    return tracks[track]["items"] if track < len(tracks) else []


def s_montage(c):
    """A short edit with the tools a real one uses, exported to H.264 1080p and checked."""
    escape(c)
    log = []
    # media: the iPhone files (Dolby Vision included) and a 1080p clip
    media = documents() / "Media"
    media.mkdir(parents=True, exist_ok=True)
    paths = []
    for name in ["iphone_dvh1.mov", "h264_1080p.mp4", "iphone_hevc_4k.mov"]:
        dst = media / f"montage-{name}"
        shutil.copyfile(FIXTURES / name, dst)
        paths.append(str(dst))
    items = c.run("file.import", {"paths": paths})["items"]
    seq = c.run("file.newSequence", {"name": "Petit montage", "width": 1920, "height": 1080, "fps": 30})["sequence"]
    # 1. three clips back to back on V1 / A1
    c.run("timeline.place", {"items": items, "track": "V1", "time": 0, "insert": False})
    v = clips(c)
    if len(v) != 3:
        raise RuntimeError(f"place: {len(v)} clips on V1")
    log.append("3 clips")
    # 2. razor at 5 s and 8 s, ripple delete the 3 s between
    # every track at once (the sound stays in sync)
    c.run("sequence.addEditAllTracks", {"time": 5 * TICKS})
    c.run("sequence.addEditAllTracks", {"time": 8 * TICKS})
    v = clips(c)
    if len(v) != 5:
        raise RuntimeError(f"razor: {len(v)} clips, expected 5")
    before = c.run("sequence.inspect")["durationFrames"]
    mid = v[1]["clip"]
    c.run("edit.rippleDelete", {"clips": [mid]})
    after = c.run("sequence.inspect")["durationFrames"]
    if before - after != 90:
        raise RuntimeError(f"ripple delete: {before} -> {after} frames, expected -90")
    log.append("lame + suppression avec propagation")
    # 3. ripple trim the out point of the first clip by 15 frames
    v = clips(c)
    c.run("timeline.trim", {"clip": v[0]["clip"], "edge": "out", "mode": "ripple", "deltaFrames": -15})
    if c.run("sequence.inspect")["durationFrames"] != after - 15:
        raise RuntimeError("ripple trim did not shorten the sequence")
    log.append("rognage")
    # 4. cross dissolve between the first two clips, constant power on the sound
    v = clips(c)
    c.run("sequence.applyVideoTransition", {"clip": v[1]["clip"], "effect": "cross_dissolve", "frames": 15, "edge": "in"})
    seqi = c.run("sequence.inspect")
    if not seqi["video"][0]["transitions"]:
        raise RuntimeError("no video transition")
    a = clips(c, "audio")
    if a:
        c.run("sequence.applyAudioTransition", {"clip": a[1]["clip"] if len(a) > 1 else a[0]["clip"], "effect": "constant_power", "frames": 15, "edge": "in"})
    log.append("fondus")
    # 5. effects: blur on clip 1, a Lumetri look on clip 2, 50 % speed on clip 3
    v = clips(c)
    c.run("effects.apply", {"clips": [v[0]["clip"]], "effect": "gaussian_blur"})
    names = [e["effect"] for e in clips(c)[0]["effects"]]
    if "gaussian_blur" not in names:
        raise RuntimeError(f"the blur was not applied: {names}")
    c.run("effects.setParam", {"clip": v[0]["clip"], "effect": names.index("gaussian_blur"), "param": "blurriness", "value": 12})
    fx = {e["effect"]: e["params"] for e in clips(c)[0]["effects"]}
    if fx["gaussian_blur"]["blurriness"]["value"] != "Float(12.0)" or fx["time_remap"]["speed"]["value"] != "Float(100.0)":
        raise RuntimeError(f"blur parameters: {fx['gaussian_blur']}, speed {fx['time_remap']['speed']}")
    look = c.run("lumetri.presets")["presets"][0]["name"]
    c.run("lumetri.applyPreset", {"name": look, "clips": [v[1]["clip"]]})
    last = v[-1]["durationFrames"]
    c.run("clip.speedDuration", {"clips": [v[-1]["clip"]], "speed": 50, "ripple": True})
    if clips(c)[-1]["durationFrames"] != 2 * last:
        raise RuntimeError(f"50 % speed: {last} -> {clips(c)[-1]['durationFrames']} frames")
    log.append(f"flou, Lumetri « {look} », vitesse 50 %")
    # 6. a title on V2 for 3 s
    c.run("playhead.set", {"seconds": 0.5})
    c.run("graphics.newText", {"text": "Mon petit film", "size": 120, "seconds": 3, "track": 1, "time": TICKS // 2})
    if not clips(c, "video", 1):
        raise RuntimeError("no title clip on V2")
    log.append("titre")
    # 7. sound: -6 dB on the first audio clip, a marker, undo / redo
    a = clips(c, "audio")
    if a:
        c.run("clip.audioGain", {"clips": [a[0]["clip"]], "mode": "set", "db": -6})
    c.run("markers.add", {"target": "program", "time": 2 * TICKS, "name": "Début"})
    n = len(c.run("sequence.inspect")["markers"])
    c.run("edit.undo")
    c.run("edit.redo")
    if len(c.run("sequence.inspect")["markers"]) != n:
        raise RuntimeError("undo / redo lost the marker")
    log.append("volume, marqueur, annuler/rétablir")
    # 8. play it, save, export H.264 1080p and check the file
    c.call("ui.playback", {"action": "play"})
    time.sleep(4)
    c.call("ui.playback", {"action": "stop"})
    shot("montage-timeline")
    proj = documents() / "Projects" / "Petit montage.fcproj"
    proj.unlink(missing_ok=True)
    c.run("file.saveAs", {"path": str(proj)})
    wait("saved project", lambda: proj.exists() and proj.stat().st_size > 0, 20)
    out = documents() / "Exports" / "Petit montage.mp4"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.unlink(missing_ok=True)
    t0 = time.time()
    # as the Export button does: in the background, the interface stays usable meanwhile
    c.run("file.exportMedia", {"path": str(out), "format": "h264", "width": 1920, "height": 1080, "fps": 30})
    slowest = 0.0
    while True:
        t = time.time()
        jobs = c.call("perf.stats").get("jobs", {})
        slowest = max(slowest, time.time() - t)
        if not jobs.get("running"):
            break
        if time.time() - t0 > 900:
            raise RuntimeError("export still running after 15 min")
        time.sleep(2)
    if slowest > 2.0:
        raise RuntimeError(f"the interface froze {slowest:.1f} s during the export")
    took = time.time() - t0
    shot("import")
    return f"{len(items)} files readable in {took:.2f} s ({sorted(set(st.values()))})"


SCENARIOS = {"menus": s_menus, "import": s_import, "playback": s_playback, "edit": s_edit, "save": s_save, "montage": s_montage}


def relaunch():
    # the simulator the run started on, not sim.sh's default
    env = dict(os.environ, FILMCRAFT_SIM=sim())
    subprocess.run([str(HERE / "sim.sh"), "--no-build"], check=True, capture_output=True, env=env)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--loop", type=int, default=1)
    a = ap.parse_args()
    names = a.names or list(SCENARIOS)
    failures = 0
    for round_ in range(1, a.loop + 1):
        if a.loop > 1:
            relaunch()
        c = Control()
        for name in names:
            t = time.time()
            try:
                info = SCENARIOS[name](c)
                print(f"[{round_}] OK   {name:9} {time.time() - t:5.1f} s  {info}", flush=True)
            except Exception as e:  # noqa: BLE001 — a scenario's failure is reported, the run goes on
                failures += 1
                shot(f"fail-{round_}-{name}")
                print(f"[{round_}] FAIL {name:9} {time.time() - t:5.1f} s  {e}", flush=True)
                c = Control()
        alive = subprocess.run(["xcrun", "simctl", "spawn", sim(), "launchctl", "list"], capture_output=True, text=True).stdout
        if bundle() not in alive:
            failures += 1
            print(f"[{round_}] FAIL app is no longer running (crash?)", flush=True)
    print(f"{failures} failure(s)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
