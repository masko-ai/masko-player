#!/usr/bin/env python3
"""
Source-clip generator for the ESP32-S3 mascot animation demo.

Renders 466x466 RGBA frames (alpha = 0 background) for four mascot states and
pipes them into ffmpeg as `rawvideo` input, producing VP9/WebM with a real
alpha channel.

All four clips share the same character design and palette so they read as one
mascot changing state.  Every animated quantity is a function of the loop
phase 2*pi*i/N with integer harmonics, so the last frame flows into the first.

Usage:
    python3 gen_assets.py            # writes idle.webm listening.webm thinking.webm speaking.webm
    python3 gen_assets.py idle       # single clip
"""
import math
import os
import subprocess
import sys

from PIL import Image, ImageDraw

W = H = 466          # output size (must be exactly 466x466)
S = 4               # supersampling factor for anti-aliasing
FPS = 24
CS = 0.92           # global character scale -> subject covers ~59% of height
FFMPEG = "/opt/homebrew/bin/ffmpeg"

# ---------------------------------------------------------------- palette ---
BODY = (86, 204, 242, 255)      # main body cyan
BODY_DARK = (44, 140, 196, 255)  # lower body / shading
OUTLINE = (23, 42, 66, 255)      # dark outline
EYE_WHITE = (255, 255, 255, 255)
PUPIL = (23, 42, 66, 255)
ACCENT = (255, 176, 46, 255)    # orange accent (rings / spinner / bars)
MOUTH = (23, 42, 66, 255)

# ------------------------------------------------------- character geometry --
# All coordinates are in final 466x466 space.
HEAD_X0, HEAD_X1 = 128, 338
HEAD_Y0, HEAD_Y1 = 104, 264
BODY_X0, BODY_X1 = 150, 316
BODY_Y0, BODY_Y1 = 252, 356
ANT_TIP = (233, 58)
EYE_L = (188, 178)
EYE_R = (278, 178)
EYE_R_OUT = 23
EYE_R_PUP = 10.5


def new_canvas():
    """Transparent RGBA canvas at supersampled resolution."""
    return Image.new("RGBA", (W * S, H * S), (0, 0, 0, 0))


def sc(v):
    return v * S


def draw_character(d: ImageDraw.ImageDraw, bob=0.0, breathe=1.0, tilt=0.0,
                   mouth_open=0.0, blink=0.0, antenna=True):
    """Draw the mascot into a supersampled ImageDraw context.

    bob       : vertical offset in final px (positive = down)
    breathe   : multiplicative scale around the character centre
    tilt      : rotation in degrees
    mouth_open: 0..1, opens the mouth
    blink     : 0..1, closes the eyes
    antenna   : draw the antenna (disabled in the thinking state)
    """
    cx, cy = 233.0, 235.0

    def T(px, py):
        """Apply global scale + tilt + breathing scale + bob to a point."""
        a = math.radians(tilt)
        dx = (px - cx) * CS
        dy = (py - cy) * CS
        rx = dx * math.cos(a) - dy * math.sin(a)
        ry = dx * math.sin(a) + dy * math.cos(a)
        rx *= breathe
        ry *= breathe
        return (sc(cx + rx), sc(cy + ry + bob))

    # ---- antenna (behind head) -------------------------------------------
    if antenna:
        ax, ay = T(ANT_TIP[0], ANT_TIP[1])
        hx0, hy0 = T(233, HEAD_Y0 + 6)
        d.line([(ax, ay), (hx0, hy0)], fill=OUTLINE, width=int(6 * S))
        d.ellipse([ax - 12 * S, ay - 12 * S, ax + 12 * S, ay + 12 * S],
                  fill=ACCENT, outline=OUTLINE, width=int(3 * S))

    # ---- body ------------------------------------------------------------
    bx0, by0 = T(BODY_X0, BODY_Y0)
    bx1, by1 = T(BODY_X1, BODY_Y1)
    d.rounded_rectangle([bx0, by0, bx1, by1], radius=int(34 * S),
                        fill=BODY_DARK, outline=OUTLINE, width=int(5 * S))
    # belly plate
    px0, py0 = T(BODY_X0 + 26, BODY_Y0 + 30)
    px1, py1 = T(BODY_X1 - 26, BODY_Y1 - 16)
    d.rounded_rectangle([px0, py0, px1, py1], radius=int(18 * S), fill=BODY)

    # ---- head ------------------------------------------------------------
    hx0, hy0 = T(HEAD_X0, HEAD_Y0)
    hx1, hy1 = T(HEAD_X1, HEAD_Y1)
    d.rounded_rectangle([hx0, hy0, hx1, hy1], radius=int(64 * S),
                        fill=BODY, outline=OUTLINE, width=int(5 * S))
    # visor
    vx0, vy0 = T(HEAD_X0 + 16, HEAD_Y0 + 24)
    vx1, vy1 = T(HEAD_X1 - 16, HEAD_Y1 - 28)
    d.rounded_rectangle([vx0, vy0, vx1, vy1], radius=int(40 * S),
                        fill=(214, 244, 255, 255))

    # ---- eyes ------------------------------------------------------------
    for (ex, ey) in (EYE_L, EYE_R):
        cxx, cyy = T(ex, ey)
        rr = EYE_R_OUT * CS * S * (1.0 - 0.88 * blink)
        d.ellipse([cxx - rr, cyy - rr, cxx + rr, cyy + rr],
                  fill=EYE_WHITE, outline=OUTLINE, width=int(2.5 * S))
        if blink < 0.95:
            pr = EYE_R_PUP * CS * S * (1.0 - 0.85 * blink)
            d.ellipse([cxx - pr, cyy - pr, cxx + pr, cyy + pr], fill=PUPIL)
            gr = 3.2 * S
            d.ellipse([cxx - pr * 0.45 - gr, cyy - pr * 0.45 - gr,
                       cxx - pr * 0.45 + gr, cyy - pr * 0.45 + gr],
                      fill=(255, 255, 255, 255))

    # ---- mouth -----------------------------------------------------------
    mx, my = T(233, HEAD_Y1 - 22)
    w = 34 * CS * S
    if mouth_open > 0.02:
        h = (4 + 20 * mouth_open) * CS * S
        d.ellipse([mx - w / 2, my - h / 2, mx + w / 2, my + h / 2], fill=MOUTH)
    else:
        d.arc([mx - w / 2, my - 13 * CS * S, mx + w / 2, my + 13 * CS * S],
              start=20, end=160, fill=MOUTH, width=int(4 * S))


def draw_rings(d: ImageDraw.ImageDraw, phase, n_rings=3, cx=233, cy=178):
    """Expanding concentric rings (listening state).

    Radii stay inside a tight annulus around the head so that the alpha
    bounding box remains close to the character itself.
    """
    r0, r1 = 74, 152
    for k in range(n_rings):
        f = (phase + k / n_rings) % 1.0
        r = r0 + (r1 - r0) * f
        fade = max(0.0, (0.88 - f) / 0.88)
        a = int(240 * fade ** 1.3)
        if a <= 3:
            continue
        w = int(max(2, 9 * fade))
        d.ellipse([sc(cx - r), sc(cy - r), sc(cx + r), sc(cy + r)],
                  outline=(ACCENT[0], ACCENT[1], ACCENT[2], a), width=w * S)


def draw_spinner(d: ImageDraw.ImageDraw, angle, cx=233, cy=72, r=21):
    """Rotating arc spinner above the head (thinking state)."""
    bbox = [sc(cx - r), sc(cy - r), sc(cx + r), sc(cy + r)]
    d.arc(bbox, start=angle, end=angle + 250, fill=ACCENT, width=int(8 * S))
    a = math.radians(angle + 250)
    tx, ty = cx + r * math.cos(a), cy + r * math.sin(a)
    d.ellipse([sc(tx - 6), sc(ty - 6), sc(tx + 6), sc(ty + 6)],
              fill=(255, 224, 150, 255))


def draw_bars(d: ImageDraw.ImageDraw, heights, cx=233, base_y=338, bw=20, gap=10):
    """Vertical waveform bars drawn on the belly (speaking state)."""
    total = len(heights) * bw + (len(heights) - 1) * gap
    x = cx - total / 2
    for h in heights:
        h = max(6.0, h)
        x0 = sc(x)
        x1 = sc(x + bw)
        y0 = sc(base_y - h)
        y1 = sc(base_y)
        d.rounded_rectangle([x0, y0, x1, y1], radius=int(bw / 2 * S),
                            fill=ACCENT, outline=OUTLINE, width=int(2.5 * S))
        x += bw + gap


# ------------------------------------------------------------- frame makers -
def frames_idle(n):
    """48 frames = 2.0 s: gentle vertical bob + slow breathing + a blink."""
    for i in range(n):
        p = 2 * math.pi * i / n
        bob = 7.0 * math.sin(p)
        breathe = 1.0 + 0.030 * math.sin(p)
        blink = 0.0
        if 30 <= i <= 33:            # short blink, fully closed for 2 frames
            blink = 1.0 if i in (31, 32) else 0.5
        img = new_canvas()
        d = ImageDraw.Draw(img)
        draw_character(d, bob=bob, breathe=breathe, tilt=1.6 * math.sin(p),
                       blink=blink)
        yield img


def frames_listening(n):
    """36 frames = 1.5 s: concentric rings expanding outward."""
    for i in range(n):
        p = 2 * math.pi * i / n
        img = new_canvas()
        d = ImageDraw.Draw(img)
        draw_rings(d, i / n)
        draw_character(d, bob=2.5 * math.sin(p), breathe=1.0,
                       tilt=1.5 * math.sin(p))
        # recording dot on the visor
        mx, my = sc(233), sc(136)
        d.ellipse([mx - 9 * S, my - 9 * S, mx + 9 * S, my + 9 * S],
                  fill=(255, 82, 82, 255), outline=OUTLINE, width=int(2.5 * S))
        yield img


def frames_thinking(n):
    """36 frames = 1.5 s: rotating arc spinner, antenna replaced by spinner."""
    for i in range(n):
        p = 2 * math.pi * i / n
        angle = -90 + 360 * i / n
        img = new_canvas()
        d = ImageDraw.Draw(img)
        draw_character(d, bob=1.5 * math.sin(p), breathe=1.0,
                       tilt=3.0 * math.sin(p), blink=0.35, antenna=False)
        draw_spinner(d, angle)
        yield img


def frames_speaking(n):
    """36 frames = 1.5 s: animated vertical waveform bars on the belly."""
    nb = 7
    for i in range(n):
        p = 2 * math.pi * i / n
        img = new_canvas()
        d = ImageDraw.Draw(img)
        draw_character(d, bob=2.0 * math.sin(p),
                       breathe=1.0 + 0.012 * math.sin(p),
                       mouth_open=0.5 + 0.5 * math.sin(p))
        heights = []
        for k in range(nb):
            # integer harmonics of the loop period -> seamless
            f = 1 + (k % 3)
            ph = k * 0.9
            h = 18.0 + 42.0 * (0.5 + 0.5 * math.sin(p * f + ph)) ** 1.3
            heights.append(h)
        draw_bars(d, heights)
        yield img


CLIPS = {
    "idle": (48, frames_idle),
    "listening": (36, frames_listening),
    "thinking": (36, frames_thinking),
    "speaking": (36, frames_speaking),
}


def encode(name, n, maker, outdir):
    out = os.path.join(outdir, name + ".webm")
    cmd = [
        FFMPEG, "-y", "-v", "error",
        "-f", "rawvideo", "-pix_fmt", "rgba", "-s", f"{W}x{H}", "-r", str(FPS),
        "-i", "-",
        "-an",
        "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p",
        "-b:v", "0", "-crf", "26", "-deadline", "good", "-cpu-used", "2",
        "-row-mt", "1", "-auto-alt-ref", "0",
        "-metadata:s:v:0", "alpha_mode=1",
        out,
    ]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    try:
        for img in maker(n):
            small = img.resize((W, H), Image.LANCZOS)
            p.stdin.write(small.tobytes())
    finally:
        p.stdin.close()
        rc = p.wait()
    if rc != 0:
        raise SystemExit(f"ffmpeg failed for {name} (rc={rc})")
    print(f"wrote {out}")


def main():
    outdir = os.path.dirname(os.path.abspath(__file__))
    names = sys.argv[1:] or list(CLIPS)
    for name in names:
        n, maker = CLIPS[name]
        encode(name, n, maker, outdir)


if __name__ == "__main__":
    main()
