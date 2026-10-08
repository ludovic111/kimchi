#!/usr/bin/env python3
"""kimchi's agent evals (lsuite HARNESS.md part 7).

Each job in jobs.json builds a project file from generated fixtures, asks kimchi's built-in agent
for a video job in words (`kimchi-cli --file cut.json ask … --json`, headless, with a real model),
then scores the project it left with automatic checks: its structure (clips, tracks, durations,
titles, transitions, markers), its numbers (loudness, true peak) and its rendered frames (not
blank, filling the frame), and whether the agent looked at its work before finishing (the finish
routine). A job passes when all its checks pass.

    evals/run.py                         # every job, Claude Code (no key needed when signed in)
    evals/run.py title-card rough-cut    # some jobs
    evals/run.py --provider anthropic --model claude-sonnet-5-5   # needs ANTHROPIC_API_KEY
    evals/run.py --record                # also add the run to evals/RESULTS.md

Needs ffmpeg, Python 3 and a built kimchi-cli (cargo build -p kimchi-cli -p kimchi-mcp; --cli
points elsewhere). Everything runs in evals/.work/ with scratch data, config and lsuite folders,
so the person's own kimchi is untouched. Run it before each release: a harness change that lowers
the pass rate doesn't ship.
"""

import argparse
import datetime
import json
import os
import shutil
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
LOOKS = {"harness.look", "project.renderFrame", "media.frame", "media.look", "ui.screenshot"}


# ---- fixtures ----------------------------------------------------------------------------------

def fixtures(folder: Path) -> Path:
    """Three distinct 8 s shots with sound, music (normal and quiet) and a photo, made once."""
    folder.mkdir(parents=True, exist_ok=True)
    made = {
        "shot-a.mp4": ["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=8", "-f", "lavfi", "-i", "sine=frequency=220:duration=8:sample_rate=48000"],
        "shot-b.mp4": ["-f", "lavfi", "-i", "mandelbrot=size=1280x720:rate=30", "-f", "lavfi", "-i", "sine=frequency=330:duration=8:sample_rate=48000", "-t", "8"],
        "shot-c.mp4": ["-f", "lavfi", "-i", "life=size=1280x720:rate=30:mold=10:ratio=0.3:death_color=#202060:life_color=#f0c040", "-f", "lavfi", "-i", "sine=frequency=440:duration=8:sample_rate=48000", "-t", "8"],
        "music.wav": ["-f", "lavfi", "-i", "aevalsrc='0.25*sin(2*PI*220*t)*(0.6+0.4*sin(2*PI*2*t))+0.2*sin(2*PI*330*t)+0.15*sin(2*PI*440*t)*gt(mod(t,0.5),0.4)':s=48000:d=30"],
        "music-quiet.wav": ["-f", "lavfi", "-i", "aevalsrc='0.02*sin(2*PI*220*t)*(0.6+0.4*sin(2*PI*2*t))+0.015*sin(2*PI*330*t)':s=48000:d=30"],
        "photo.png": ["-f", "lavfi", "-i", "smptehdbars=size=1920x1080", "-frames:v", "1"],
    }
    for name, args in made.items():
        out = folder / name
        if out.exists():
            continue
        extra = ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"] if name.endswith(".mp4") else []
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args, *extra, str(out)], check=True)
    return folder


# ---- kimchi --------------------------------------------------------------------------------------

class Kimchi:
    def __init__(self, cli: Path, home: Path):
        self.cli = cli
        self.env = dict(os.environ)
        self.env.update({
            "KIMCHI_DATA_DIR": str(home / "data"),
            "KIMCHI_CONFIG_DIR": str(home / "config"),
            "LSUITE_HOME": str(home / "lsuite"),
            "KIMCHI_NO_UPDATE": "1",
            "KIMCHI_MCP": str(cli.with_name("kimchi-mcp")),
        })

    def call(self, project: Path, command: str, params: dict | None = None, timeout=600):
        r = subprocess.run([str(self.cli), "--file", str(project), "--compact", command, "--args", json.dumps(params or {})],
                           env=self.env, capture_output=True, text=True, timeout=timeout)
        if r.returncode != 0:
            raise RuntimeError(f"{command}: {r.stderr.strip()}")
        return json.loads(r.stdout)

    def ask(self, project: Path, prompt: str, provider: str, model: str | None, log: Path, timeout: int):
        args = [str(self.cli), "--file", str(project), "ask", prompt, "--provider", provider, "--json"]
        if model:
            args += ["--model", model]
        started = time.time()
        events, error = [], None
        with open(log, "w") as f:
            try:
                p = subprocess.run(args, env=self.env, capture_output=True, text=True, timeout=timeout)
                f.write(p.stdout)
                f.write("\n# stderr\n" + p.stderr)
                for line in p.stdout.splitlines():
                    try:
                        events.append(json.loads(line))
                    except json.JSONDecodeError:
                        pass
                if p.returncode != 0:
                    error = (p.stderr.strip().splitlines() or ["failed"])[-1]
            except subprocess.TimeoutExpired:
                error = f"timed out after {timeout} s"
        return events, error, time.time() - started


# ---- checks --------------------------------------------------------------------------------------

def clips(p, kind=None):
    for t in p["tracks"]:
        if kind and t["kind"] != kind:
            continue
        for c in t["clips"]:
            yield t, c


def asset(p, c):
    aid = c["content"].get("asset_id")
    return next((a for a in p["assets"] if a["id"] == aid), None)


def asset_stem(p, c):
    a = asset(p, c)
    return Path(a["path"]).stem if a else None


def text_of(c):
    """The words a clip shows: a title's, or anything in a motion clip's scene and template."""
    content = c["content"]
    if content["type"] == "text":
        return content["style"]["content"]
    if content["type"] == "motion":
        return json.dumps(content, ensure_ascii=False)
    return ""


def media_clips(p, kinds=("video", "image")):
    return [(t, c) for t, c in clips(p, "video") if c["content"]["type"] == "media" and (asset(p, c) or {}).get("kind") in kinds]


def end_of(p):
    return max((c["start"] + c["duration"] for _, c in clips(p)), default=0.0)


class Scorer:
    def __init__(self, kimchi: Kimchi, project: Path, events: list):
        self.k, self.file, self.events = kimchi, project, events
        self.p = kimchi.call(project, "project.get")
        self._look = None

    def look(self):
        if self._look is None:
            self._look = self.k.call(self.file, "harness.look", {"frames": 8})
        return self._look

    def texts(self, text):
        return [(t, c) for t, c in clips(self.p, "video") if text.lower() in text_of(c).lower()]

    # Each check returns (passed, detail).
    def text_contains(self, text, start_min=None, start_max=None, min_duration=None):
        found = self.texts(text)
        if not found:
            return False, f"no title or motion clip says {text!r}"
        _, c = found[0]
        if start_min is not None and c["start"] < start_min - 1e-6:
            return False, f"starts at {c['start']:.2f} s, before {start_min}"
        if start_max is not None and c["start"] > start_max + 1e-6:
            return False, f"starts at {c['start']:.2f} s, after {start_max}"
        if min_duration is not None and c["duration"] < min_duration - 1e-6:
            return False, f"lasts {c['duration']:.2f} s, under {min_duration}"
        return True, f"{c['content']['type']} at {c['start']:.2f} s for {c['duration']:.2f} s"

    def text_fades(self, text):
        found = self.texts(text)
        if not found:
            return False, "no such title"
        _, c = found[0]
        kf = c.get("keyframes", {})
        if c.get("fade_in", 0) > 0 and c.get("fade_out", 0) > 0:
            return True, f"fades {c['fade_in']} / {c['fade_out']} s"
        if "opacity" in kf and len(kf["opacity"]) >= 3:
            return True, "opacity keyframes in and out"
        if c["content"]["type"] == "motion" and c.get("fade_out", 0) > 0:
            return True, "animated scene with a fade out"
        return False, f"fade_in {c.get('fade_in', 0)}, fade_out {c.get('fade_out', 0)}, keyframes {sorted(kf)}"

    def above_picture(self, text):
        order = [i for i, t in enumerate(self.p["tracks"]) if any(text.lower() in text_of(c).lower() for c in t["clips"])]
        pictures = [i for i, t in enumerate(self.p["tracks"]) if any(c["content"]["type"] == "media" for c in t["clips"]) and t["kind"] == "video"]
        if not order or not pictures:
            return False, "the text or the picture is missing"
        return order[0] < min(pictures), f"text on track {order[0]}, picture on track {min(pictures)}"

    def canvas(self, width, height):
        s = self.p["settings"]
        return (s["width"], s["height"]) == (width, height), f"{s['width']}×{s['height']}"

    def duration(self, min, max):
        d = end_of(self.p)
        return min <= d <= max, f"{d:.2f} s"

    def video_clips(self, min, distinct_assets=None):
        m = media_clips(self.p)
        distinct = {asset_stem(self.p, c) for _, c in m}
        ok = len(m) >= min and (distinct_assets is None or len(distinct) >= distinct_assets)
        return ok, f"{len(m)} clips from {len(distinct)} media"

    def order(self, assets):
        seq = [asset_stem(self.p, c) for _, c in sorted(media_clips(self.p), key=lambda tc: tc[1]["start"])]
        firsts = [seq.index(a) for a in assets if a in seq]
        return len(firsts) == len(assets) and firsts == sorted(firsts), " → ".join(s or "?" for s in seq)

    def no_gaps(self):
        gaps = self.look().get("gaps", [])
        return not gaps, f"gaps {gaps}" if gaps else "none"

    def frames_nonblank(self, times=None):
        look = self.k.call(self.file, "harness.look", {"times": times, "measure": False}) if times else self.look()
        blank = [f["time"] for f in look["frames"] if f["blank"]]
        return not blank, f"blank at {blank}" if blank else f"{len(look['frames'])} frames with something in them"

    def fills_frame(self, time):
        """No black bars: the top and bottom bands of a rendered frame aren't black."""
        r = self.k.call(self.file, "project.renderFrame", {"time": time, "width": 270})
        w, h, rows = read_png(Path(r["path"]))
        band = max(2, h // 20)

        def mean(rs):
            vals = [0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2] for row in rs for px in row]
            return sum(vals) / len(vals)

        top, bottom = mean(rows[:band]), mean(rows[-band:])
        return top > 12 and bottom > 12, f"top band {top:.0f}, bottom band {bottom:.0f} (0–255)"

    def all_video_clips_warm(self):
        m = media_clips(self.p)
        temps = [c.get("effects", {}).get("temperature", 0) for _, c in m]
        return bool(m) and all(t > 0.02 for t in temps), f"temperatures {temps}"

    def transitions(self, kind, count, duration, tolerance):
        got = [c["transition"] for _, c in clips(self.p, "video") if c.get("transition")]
        good = [t for t in got if t["kind"] == kind and abs(t["duration"] - duration) <= tolerance]
        return len(good) >= count, f"{[(t['kind'], t['duration']) for t in got]}"

    def loudness(self, target, tolerance):
        master = (self.p.get("mixer") or {}).get("master") or {}
        if master.get("loudness") is not None and abs(master["loudness"] - target) <= tolerance:
            return True, f"exports brought to {master['loudness']} LUFS (master target)"
        m = self.k.call(self.file, "audio.measure", {})
        i = m.get("integrated")
        return i is not None and abs(i - target) <= tolerance, f"mix {i} LUFS, master target {master.get('loudness')}"

    def true_peak(self, max):
        master = (self.p.get("mixer") or {}).get("master") or {}
        if master.get("loudness") is not None and master.get("limiter", True) and master.get("ceilingDb", -1) <= max:
            return True, f"limiter on at {master.get('ceilingDb', -1)} dBTP"
        m = self.k.call(self.file, "audio.measure", {})
        tp = m.get("truePeak")
        return tp is not None and tp <= max, f"true peak {tp} dBTP"

    def music_ends_with_picture(self, fade_min):
        pic_end = max((c["start"] + c["duration"] for _, c in media_clips(self.p)), default=0)
        music = [c for _, c in clips(self.p, "audio")]
        if not music:
            return False, "no music left"
        last = max(music, key=lambda c: c["start"] + c["duration"])
        end = last["start"] + last["duration"]
        fade = last.get("fade_out", 0)
        kf = last.get("keyframes", {}).get("volume", [])
        faded = fade >= fade_min or (len(kf) >= 2 and kf[-1]["value"] <= 0.05)
        return end <= pic_end + 0.15 and faded, f"music ends {end:.2f} s (picture {pic_end:.2f} s), fade out {fade}"

    def markers(self, labels, times, tolerance):
        ms = sorted(self.p.get("markers", []), key=lambda m: m["time"])
        ok = all(any(m["label"].lower() == l.lower() and abs(m["time"] - t) <= tolerance for m in ms) for l, t in zip(labels, times))
        return ok, f"{[(m['label'], m['time']) for m in ms]}"

    def motion_clip(self, three_d, text):
        for _, c in clips(self.p, "video"):
            if c["content"]["type"] != "motion":
                continue
            scene = c["content"].get("scene", {})
            is3d = scene.get("type") == "3d"
            if is3d == three_d and text.lower() in json.dumps(c["content"]).lower():
                return True, f"{'3D' if is3d else '2D'} motion clip at {c['start']:.2f} s"
        return False, "no such motion clip"

    def image_clip(self, min_duration, max_duration):
        m = media_clips(self.p, kinds=("image",))
        if not m:
            return False, "no picture on the timeline"
        c = m[0][1]
        return min_duration <= c["duration"] <= max_duration and c["start"] <= 0.25, f"at {c['start']:.2f} s for {c['duration']:.2f} s"

    def keyframed(self, properties):
        for _, c in clips(self.p, "video"):
            kf = c.get("keyframes", {})
            hit = [k for k in properties if len(kf.get(k, [])) >= 2]
            if hit:
                return True, f"{c['name']}: {hit}"
        return False, "no clip moves"

    def looked(self, sound=False):
        """The finish routine: a look (frames, a sheet) after the last change; for a sound job,
        measuring the mix (`audio.measure`) is checking it too."""
        checks = LOOKS | {"audio.measure"} if sound else LOOKS
        cmds = [e["record"] for e in self.events if e.get("type") == "command"]
        last_edit = max((i for i, r in enumerate(cmds) if r["ok"] and r["mutates"] and r["command"] not in checks), default=-1)
        after = [r["command"] for r in cmds[last_edit + 1:] if r["ok"] and r["command"] in checks]
        return bool(after), f"looked with {after}" if after else "no look after the last change"


def read_png(path: Path):
    """A PNG's size and RGB rows, without Pillow (8-bit RGB or RGBA, non-interlaced)."""
    data = path.read_bytes()
    pos, idat, w = 8, b"", 0
    while pos < len(data):
        n, kind = struct.unpack(">I4s", data[pos:pos + 8])
        chunk = data[pos + 8:pos + 8 + n]
        if kind == b"IHDR":
            w, h, depth, color = struct.unpack(">IIBB", chunk[:10])
            channels = {2: 3, 6: 4}[color]
        elif kind == b"IDAT":
            idat += chunk
        pos += 12 + n
    raw = zlib.decompress(idat)
    stride = w * channels
    rows, prev = [], bytearray(stride)
    for y in range(h):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append([line[x * channels:x * channels + 3] for x in range(w)])
        prev = line
    return w, h, rows


# ---- running -------------------------------------------------------------------------------------

def run_job(k: Kimchi, job: dict, fx: Path, work: Path, provider: str, model: str | None, timeout: int):
    folder = work / job["name"]
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    project = folder / "cut.json"
    w, h = job.get("canvas", [1920, 1080])
    k.call(project, "project.create", {"name": job["name"], "width": w, "height": h})
    try:
        for step in job.get("setup", []):
            params = json.loads(json.dumps(step["params"]).replace("{fx}", str(fx)))
            k.call(project, step["command"], params)
    except Exception as e:  # a job whose setup fails is a failed job, not a stopped run
        return {"job": job["name"], "passed": False, "checks": [{"check": "setup", "ok": False, "detail": str(e)}],
                "seconds": 0, "commands": 0, "tokens": 0, "reply": "", "error": str(e)}
    events, error, seconds = k.ask(project, job["prompt"], provider, model, folder / "run.jsonl", timeout)
    done = next((e for e in events if e.get("type") == "done"), None)
    tokens = sum(e.get("input_tokens", 0) + e.get("output_tokens", 0) for e in events if e.get("type") == "usage")
    commands = [e["record"]["command"] for e in events if e.get("type") == "command"]
    results = []
    if done is None:
        results.append(("run", False, error or "the run didn't finish"))
    scorer = Scorer(k, project, events)
    for name, params in job["checks"]:
        try:
            ok, detail = getattr(scorer, name)(**params)
        except Exception as e:  # a check that can't run is a failed check
            ok, detail = False, f"check failed to run: {e}"
        results.append((name, bool(ok), detail))
    return {
        "job": job["name"],
        "passed": all(ok for _, ok, _ in results),
        "checks": [{"check": n, "ok": ok, "detail": d} for n, ok, d in results],
        "seconds": round(seconds),
        "commands": len(commands),
        "tokens": tokens,
        "reply": (done or {}).get("summary", "")[:400],
        "error": error,
    }


def record(results: list, provider: str, model: str | None, out: Path):
    passed = sum(r["passed"] for r in results)
    date = datetime.date.today().isoformat()
    lines = [
        f"## {date} · {provider}{' · ' + model if model else ''} · {passed}/{len(results)} passed",
        "",
        "| Job | Result | Commands | Time | Failed checks |",
        "| --- | --- | --- | --- | --- |",
    ]
    for r in results:
        failed = "; ".join(f"{c['check']}: {c['detail']}" for c in r["checks"] if not c["ok"]) or "—"
        lines.append(f"| {r['job']} | {'pass' if r['passed'] else 'FAIL'} | {r['commands']} | {r['seconds']} s | {failed.replace('|', '/')} |")
    lines.append("")
    text = out.read_text() if out.exists() else "# kimchi eval results\n\n"
    head, _, rest = text.partition("\n## ")
    out.write_text(head.rstrip() + "\n\n" + "\n".join(lines) + ("\n## " + rest if rest else ""))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("jobs", nargs="*", help="job names (default: all)")
    ap.add_argument("--provider", default="claude-code")
    ap.add_argument("--model", default=None)
    ap.add_argument("--cli", default=str(ROOT / "target" / "debug" / "kimchi-cli"))
    ap.add_argument("--timeout", type=int, default=900, help="seconds per job")
    ap.add_argument("--record", action="store_true", help="add the run to evals/RESULTS.md")
    a = ap.parse_args()

    spec = json.loads((HERE / "jobs.json").read_text())
    jobs = [j for j in spec["jobs"] if not a.jobs or j["name"] in a.jobs]
    unknown = set(a.jobs) - {j["name"] for j in spec["jobs"]}
    if unknown:
        sys.exit(f"unknown jobs: {', '.join(sorted(unknown))}")
    cli = Path(a.cli).resolve()
    if not cli.exists():
        sys.exit(f"{cli} isn't built: cargo build -p kimchi-cli -p kimchi-mcp")
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    work = HERE / ".work" / stamp
    k = Kimchi(cli, work / "home")
    fx = fixtures(HERE / ".work" / "fixtures")
    results = []
    for job in jobs:
        print(f"· {job['name']}…", flush=True)
        r = run_job(k, job, fx, work, a.provider, a.model, a.timeout)
        results.append(r)
        print(f"  {'pass' if r['passed'] else 'FAIL'} in {r['seconds']} s, {r['commands']} commands", flush=True)
        for c in r["checks"]:
            print(f"    {'ok ' if c['ok'] else '!! '} {c['check']}: {c['detail']}", flush=True)
    (work / "results.json").write_text(json.dumps({"provider": a.provider, "model": a.model, "results": results}, indent=2))
    passed = sum(r["passed"] for r in results)
    print(f"\n{passed}/{len(results)} passed ({work / 'results.json'})")
    if a.record:
        record(results, a.provider, a.model, HERE / "RESULTS.md")
    sys.exit(0 if passed == len(results) else 1)


if __name__ == "__main__":
    main()
