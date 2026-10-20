#!/usr/bin/env python3
"""Re-encodes an asciicast v3 at a capped frame rate: replays it through pyte
and writes, every 1/fps seconds, only the cells that changed since the last
written frame. Keeps the header. Usage: cast_cap_fps.py in.cast out.cast [fps]"""
import json, sys
import pyte

src, dst = sys.argv[1], sys.argv[2]
fps = float(sys.argv[3]) if len(sys.argv) > 3 else 25.0
lines = open(src).read().splitlines()
head = json.loads(lines[0])
cols, rows = head["term"]["cols"], head["term"]["rows"]
scr = pyte.Screen(cols, rows)
st = pyte.Stream(scr)

NAMES = ["black", "red", "green", "brown", "blue", "magenta", "cyan", "white"]
BRIGHT = ["brightblack", "brightred", "brightgreen", "brightbrown", "brightblue",
          "brightmagenta", "brightcyan", "brightwhite"]

def colour(c, fg):
    if c == "default":
        return "39" if fg else "49"
    if c in NAMES:
        return str((30 if fg else 40) + NAMES.index(c))
    if c in BRIGHT:
        return str((90 if fg else 100) + BRIGHT.index(c))
    if c == "yellow":  # pyte's name for bright brown in some versions
        return str((90 if fg else 100) + 3)
    if len(c) == 6:
        r, g, b = int(c[:2], 16), int(c[2:4], 16), int(c[4:], 16)
        return f"{38 if fg else 48};2;{r};{g};{b}"
    return "39" if fg else "49"

def sgr(ch):
    p = ["0", colour(ch.fg, True), colour(ch.bg, False)]
    if ch.bold: p.append("1")
    if ch.italics: p.append("3")
    if ch.underscore: p.append("4")
    if ch.reverse: p.append("7")
    return "\x1b[" + ";".join(p) + "m"

def key(ch):
    return (ch.data, ch.fg, ch.bg, ch.bold, ch.italics, ch.underscore, ch.reverse)

shown = {}  # (x, y) -> key
def frame():
    out, last = [], None
    for y in range(rows):
        line = scr.buffer[y]
        x = 0
        while x < cols:
            ch = line[x]
            k = key(ch)
            if shown.get((x, y)) != k:
                shown[(x, y)] = k
                s = sgr(ch)
                out.append(f"\x1b[{y + 1};{x + 1}H")
                if s != last:
                    out.append(s); last = s
                out.append(ch.data or " ")
            x += 1
    return "".join(out)

out = [json.dumps(head)]
# start in the same state as a fresh terminal: the alternate screen, cursor hidden
start = "\x1b[?1049h\x1b[?25l\x1b[2J"
t = 0.0
next_at = 0.0
written_t = 0.0
first = True
for l in lines[1:]:
    d, kind, data = json.loads(l)
    t += d
    if kind != "o":
        continue
    if "\x1b[?1049l" in data:  # the program left its screen: the preview ends here
        break
    st.feed(data)
    if t >= next_at:
        f = frame()
        if first:
            f = start + f; first = False
        if f:
            out.append(json.dumps([round(t - written_t, 3), "o", f]))
            written_t = t
        next_at = t + 1.0 / fps
f = frame()
if f:
    out.append(json.dumps([round(t - written_t, 3), "o", f]))
    written_t = t
open(dst, "w").write("\n".join(out) + "\n")
