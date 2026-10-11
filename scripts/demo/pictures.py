"""Draw Dad's made-up tomatoes photo for the demo with ImageMagick.

    python3 scripts/demo/pictures.py <tomatoes.jpg>

Simple shapes and gradients, so it reads well even as the TUI's half-block
thumbnail.
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


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as work:
        tomatoes(sys.argv[1], Path(work))
