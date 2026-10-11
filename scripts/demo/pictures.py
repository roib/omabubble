"""Draw the demo's two made-up photos with ImageMagick.

    python3 scripts/demo/pictures.py <tomatoes.jpg> <lake.jpg>

Simple shapes and gradients, so they read well even as the TUI's half-block
thumbnails.
"""

import subprocess
import sys
import tempfile
from pathlib import Path


def magick(*args):
    subprocess.run(["magick", *map(str, args)], check=True)


def tomatoes(out, work):
    """Dad's tomatoes: ripe fruit hanging among leaves, above the soil."""
    leaves = [
        (90, 120, 70, 30, 20), (260, 80, 80, 28, -25), (470, 150, 75, 26, 35),
        (690, 70, 85, 30, -15), (930, 130, 80, 28, 25), (1110, 90, 70, 26, -30),
        (150, 300, 85, 30, 15), (380, 250, 70, 24, -35), (620, 230, 80, 28, 20),
        (1000, 280, 85, 30, -20), (60, 520, 75, 26, 30), (1140, 500, 80, 28, -15),
        (240, 640, 70, 24, -25), (950, 650, 80, 28, 25),
    ]
    fruit = [(330, 430, 80), (570, 360, 72), (800, 460, 82), (480, 590, 70), (710, 620, 74)]

    args = ["-size", "1200x900", "gradient:#8cbf62-#3d6e2f", "-fill", "#4f8a36"]
    for x, y, rx, ry, angle in leaves:
        args += ["-draw", f"translate {x},{y} rotate {angle} ellipse 0,0 {rx},{ry} 0,360"]
    args += ["-stroke", "#3a6a28", "-strokewidth", "9"]
    for x, y, r in fruit:
        args += ["-draw", f"line {x},{max(0, y - 380)} {x},{y - r}"]
    args += ["-stroke", "none", "-blur", "0x2",
             "-fill", "#6b4a2e", "-draw", "rectangle 0,720 1200,900"]
    bg = work / "bg.png"
    magick(*args, bg)

    for x, y, r in fruit:
        d = r * 2
        ball = work / f"tomato{r}.png"
        # Radial shading cut to a circle.
        magick("-size", f"{d}x{d}", "radial-gradient:#ff7357-#a3170f",
               "(", "-size", f"{d}x{d}", "xc:black", "-fill", "white",
               "-draw", f"circle {r},{r} {r},1", ")",
               "-alpha", "off", "-compose", "copy_opacity", "-composite", ball)
        hx, hy = x - r // 3, y - r // 3
        magick(bg, ball, "-geometry", f"+{x - r}+{y - r}", "-composite",
               "-fill", "#3f7d2c", "-draw", f"ellipse {x},{y - r + 6} {r // 3},{r // 7} 0,360",
               "-fill", "#ffffff44", "-draw", f"circle {hx},{hy} {hx},{hy - r // 6}", bg)
    magick(bg, "-quality", "88", out)


def lake(out):
    """Jordan's lake: mountains at sunset over still water."""
    magick(
        "-size", "1200x800", "gradient:#6aa9e0-#fbd6a0",
        "(", "-size", "760x760", "radial-gradient:#fff6d0-#fff6d000", ")",
        "-geometry", "+360+0", "-composite",
        "-fill", "#fff8de", "-draw", "circle 740,330 740,288",
        "-fill", "#7d8fae",
        "-draw", "polygon 0,500 180,330 320,420 520,240 700,400 880,280 1050,380 1200,320 1200,560 0,560",
        "-fill", "#eef3fa",
        "-draw", "polygon 520,240 470,300 505,292 540,312 575,290",
        "-draw", "polygon 880,280 845,322 880,315 910,330",
        "-fill", "#4d6a55",
        "-draw", "polygon 0,560 0,470 140,430 300,480 470,440 650,500 820,450 1000,490 1200,450 1200,560",
        "(", "-size", "1200x260", "gradient:#4f86b3-#1f4766", ")",
        "-geometry", "+0+540", "-composite",
        "-fill", "#2e4a39",
        "-draw", "polygon 60,545 80,470 100,545", "-draw", "polygon 110,545 135,450 160,545",
        "-draw", "polygon 1040,545 1065,460 1090,545", "-draw", "polygon 1095,545 1115,480 1135,545",
        "-fill", "#fff8de66",
        "-draw", "rectangle 690,580 790,585", "-draw", "rectangle 705,620 775,624",
        "-draw", "rectangle 720,660 760,663",
        "-blur", "0x1.2", "-quality", "88", out,
    )


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as work:
        tomatoes(sys.argv[1], Path(work))
    lake(sys.argv[2])
