"""Makes a sample photo with ffmpeg and a session that edits it, for trying
grace9000 (and for screenshots).

    python3 scripts/sample.py <folder>

Then start grace9000 with GRACE9000_CONFIG_DIR=<folder>/config.
"""
import json
import os
import subprocess
import sys

out = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "sample")
config = os.path.join(out, "config")
os.makedirs(config, exist_ok=True)
photo = os.path.join(out, "dusk.png")

# A dusk sky over two ridges and a low sun, a little dull and dark, so the
# edit has something to do.
W, H = 3000, 2000
sky = "(1-Y/H)"
ridge1 = "H*(0.62+0.06*sin(X/W*7.0)+0.025*sin(X/W*29.0))"
ridge2 = "H*(0.74+0.05*sin(X/W*4.3+1.7)+0.02*sin(X/W*41.0))"
sun = "exp(-(pow(X-W*0.68,2)+pow(Y-H*0.55,2))/(2*pow(H*0.035,2)))"
glow = "exp(-(pow(X-W*0.68,2)+pow(Y-H*0.55,2))/(2*pow(H*0.25,2)))"


def channel(top, low, near, far, sunc):
    s = f"({low}+({top}-{low})*{sky}+{sunc}*(0.6*{glow}+{sun}))"
    return f"255*if(gt(Y,{ridge2}),{near},if(gt(Y,{ridge1}),{far},min(1,{s})))*(0.92+0.08*random(1))"


geq = ":".join([
    "r='" + channel(0.16, 0.62, 0.05, 0.14, 0.55) + "'",
    "g='" + channel(0.18, 0.36, 0.06, 0.12, 0.40) + "'",
    "b='" + channel(0.34, 0.30, 0.09, 0.20, 0.18) + "'",
])
subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"color=black:s={W}x{H}:d=1,format=rgb24,geq={geq}", "-frames:v", "1", photo], check=True)


def step(kind, on=True, points=None, **values):
    s = {"kind": kind, "on": on, "values": values}
    if points:
        s["points"] = points
    return s


session = {
    "grace9000": 1,
    "photo": photo,
    "edit": {
        "steps": [
            step("exposure", ev=0.35),
            step("tone", highlights=-25.0, shadows=40.0),
            step("white balance", temperature=18.0, tint=4.0),
            step("curve", points=[[0.0, 0.0], [0.25, 0.2], [0.75, 0.82], [1.0, 1.0]]),
            step("color", saturation=8.0, vibrance=30.0),
            step("vignette", amount=-28.0, size=55.0, feather=60.0),
        ],
        "straighten": 1.5,
        "crop": {"x": 0.04, "y": 0.06, "w": 0.92, "h": 0.86},
    },
}
with open(os.path.join(config, "session.json"), "w") as f:
    json.dump(session, f)
print(config)
