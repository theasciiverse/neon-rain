# Neon Rain

A rainy city street at night, seen from straight above, in the terminal:
rooftops with lit skylights, neon signs that buzz and lose letters, a wet
road that mirrors them, cars with headlight cones, a police car now and
then, people under umbrellas, steam from the manholes, rain streaks and
the odd lightning flash.

Inspired by the GIF of the unreleased Terminal Rain roguelike (TIGSource
devlog, 2015 to 2018); written from scratch, none of its art or code.

    cargo run --release            # any key quits
    cargo run --release -- -s 7    # the same street every time

Truecolor when the terminal says so (`COLORTERM=truecolor` or `24bit`,
Windows Terminal, Windows), 256 colours otherwise. The street is rebuilt
when the window is resized.

Session log: `RUST_LOG=debug neon-rain 2>neon-rain.log` (start, every key
and resize, lightning, a snapshot each second).
