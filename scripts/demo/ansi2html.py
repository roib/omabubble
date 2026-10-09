#!/usr/bin/env python3
"""Turn `tmux capture-pane -e -p` output into a standalone HTML terminal window.

Usage: ansi2html.py < capture.txt > shot.html
Colors are the Kanagawa palette (Omarchy's default foot theme).
"""

import html
import re
import sys

from wcwidth import wcwidth

FG, BG = "#dcd7ba", "#1f1f28"
PALETTE = [
    "#1f1f28", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#dcd7ba",
    "#54546d", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba",
]


def color256(n):
    if n < 16:
        return PALETTE[n]
    if n < 232:
        n -= 16
        steps = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (steps[n // 36], steps[n // 6 % 6], steps[n % 6])
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


class Pen:
    def __init__(self):
        self.reset()

    def reset(self):
        self.fg = self.bg = None
        self.bold = self.italic = self.underline = self.reverse = self.dim = False

    def apply(self, params):
        codes = [int(p) if p else 0 for p in params.split(";")] if params else [0]
        i = 0
        while i < len(codes):
            c = codes[i]
            if c == 0:
                self.reset()
            elif c == 1:
                self.bold = True
            elif c == 2:
                self.dim = True
            elif c == 3:
                self.italic = True
            elif c == 4:
                self.underline = True
            elif c == 7:
                self.reverse = True
            elif c == 22:
                self.bold = self.dim = False
            elif c == 23:
                self.italic = False
            elif c == 24:
                self.underline = False
            elif c == 27:
                self.reverse = False
            elif 30 <= c <= 37:
                self.fg = PALETTE[c - 30]
            elif 90 <= c <= 97:
                self.fg = PALETTE[c - 90 + 8]
            elif 40 <= c <= 47:
                self.bg = PALETTE[c - 40]
            elif 100 <= c <= 107:
                self.bg = PALETTE[c - 100 + 8]
            elif c == 39:
                self.fg = None
            elif c == 49:
                self.bg = None
            elif c in (38, 48) and i + 1 < len(codes):
                if codes[i + 1] == 5 and i + 2 < len(codes):
                    col = color256(codes[i + 2])
                    i += 2
                elif codes[i + 1] == 2 and i + 4 < len(codes):
                    col = "#%02x%02x%02x" % tuple(codes[i + 2 : i + 5])
                    i += 4
                else:
                    col = None
                if c == 38:
                    self.fg = col
                else:
                    self.bg = col
            i += 1

    def css(self):
        fg, bg = self.fg or FG, self.bg
        if self.reverse:
            fg, bg = bg or BG, fg
        out = [f"color:{fg}"]
        if bg:
            out.append(f"background:{bg}")
        if self.bold:
            out.append("font-weight:bold")
        if self.italic:
            out.append("font-style:italic")
        if self.underline:
            out.append("text-decoration:underline")
        if self.dim:
            out.append("opacity:.6")
        return ";".join(out)


# SGR sequences (kept), other CSI sequences (dropped), or one character.
TOKEN = re.compile(r"\x1b\[([0-9;]*)m|\x1b\[[0-9;?]*[A-Za-z]|(.)", re.S)


ZWJ, VS16 = "\u200d", "\ufe0f"


def is_ri(ch):
    return "\U0001f1e6" <= ch <= "\U0001f1ff"


def cells(line, pen):
    """Split a line into (style, cluster, width) cells, as a terminal does:
    variation selectors, joiners, skin tones and flag pairs stay together."""
    out = []
    for m in TOKEN.finditer(line):
        if m.group(1) is not None:
            pen.apply(m.group(1))
            continue
        ch = m.group(2)
        if ch is None:
            continue
        if out:
            prev = out[-1][1]
            joins = (
                wcwidth(ch) == 0
                or "\U0001f3fb" <= ch <= "\U0001f3ff"
                or prev.endswith(ZWJ)
                or (is_ri(ch) and len(prev) == 1 and is_ri(prev))
            )
            if joins:
                out[-1][1] += ch
                continue
        out.append([pen.css(), ch])
    result = []
    for css, cl in out:
        w = 2 if (VS16 in cl or ZWJ in cl or is_ri(cl[0])) else max(wcwidth(cl[0]), 0)
        result.append((css, cl, w))
    return result


def line_html(line, pen):
    out = []
    for css, cl, w in cells(line, pen):
        cls = ' class="w"' if w == 2 else ""
        out.append(f'<span{cls} style="{css}">{html.escape(cl)}</span>')
    return "".join(out)


def main():
    lines = sys.stdin.read().rstrip("\n").split("\n")
    pen = Pen()
    # No newlines between rows: inside `white-space: pre` they'd add blank lines.
    body = "".join(f"<div>{line_html(l, pen) or '&nbsp;'}</div>" for l in lines)
    print(f"""<!doctype html>
<meta charset="utf-8">
<style>
  html, body {{ margin: 0; background: transparent; }}
  .win {{ display: inline-block; margin: 24px; padding: 14px 16px; background: {BG};
          border-radius: 10px; box-shadow: 0 8px 28px rgba(0,0,0,.45); }}
  .term {{ font: 14px/1.25 "JetBrainsMono Nerd Font", "Noto Color Emoji", monospace;
           white-space: pre; color: {FG}; }}
  .term div {{ height: 1.25em; }}
  .w {{ display: inline-block; width: 2ch; text-align: center; }}
</style>
<div class="win"><div class="term">{body}</div></div>""")


if __name__ == "__main__":
    main()
