//! Neon Rain: a rainy city street at night, seen from straight above.
//!
//! Rooftops with lit skylights, neon signs that buzz and drop letters, a wet
//! road that mirrors them, cars with headlight cones, people under umbrellas,
//! rain streaks and the odd lightning flash. Any key quits.

use std::io::{self, BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{self, Event, KeyEventKind},
    queue,
    style::{Color, Print, SetBackgroundColor, SetForegroundColor},
    terminal,
};

const FPS: f32 = 30.0;

type Rgb = [f32; 3];

fn add(a: Rgb, b: Rgb) -> Rgb {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: Rgb, b: Rgb) -> Rgb {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
fn scale(a: Rgb, k: f32) -> Rgb {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn lum(a: Rgb) -> f32 {
    a[0].max(a[1]).max(a[2])
}

// ---------------------------------------------------------------- palette

const AMBIENT: Rgb = [0.13, 0.13, 0.27];
const FLASH: Rgb = [0.75, 0.8, 1.1];
const NEON: [Rgb; 7] = [
    [1.0, 0.18, 0.55], // pink
    [0.2, 0.9, 1.0],   // cyan
    [0.85, 0.3, 1.0],  // violet
    [1.0, 0.6, 0.15],  // amber
    [0.3, 1.0, 0.5],   // green
    [1.0, 0.2, 0.2],   // red
    [0.45, 0.5, 1.0],  // blue
];
const SIGNS: &[&str] = &[
    "NOODLES", "24H MART", "HOTEL", "BAR", "SUSHI", "PAWN", "ARCADE", "RAMEN", "CLINIC",
    "KARAOKE", "CAFE", "LAUNDRY", "TATTOO", "OPEN", "NET CAFE", "PHARMACY", "LIQUOR", "DUMPLINGS",
    "MOTEL", "REPAIRS", "CASH", "DINER", "PACHINKO", "NAILS",
];
const PAINT: [Rgb; 7] = [
    [0.15, 0.2, 0.95],
    [0.5, 0.15, 0.9],
    [0.85, 0.85, 0.9],
    [0.1, 0.6, 0.7],
    [0.75, 0.1, 0.2],
    [0.2, 0.2, 0.25],
    [0.9, 0.6, 0.1],
];
const UMBRELLAS: [Rgb; 6] = [
    [0.95, 0.2, 0.3],
    [0.95, 0.85, 0.2],
    [0.2, 0.8, 0.95],
    [0.9, 0.9, 0.95],
    [0.6, 0.3, 0.95],
    [0.3, 0.3, 0.35],
];

// ---------------------------------------------------------------- tiles

#[derive(Clone, Copy, PartialEq)]
enum Mat {
    Roof,
    Parapet,
    Facade,
    Alley,
    Sidewalk,
    Curb,
    Road,
    Floor,
}

#[derive(Clone, Copy)]
struct Tile {
    mat: Mat,
    /// A fixed mark (lane paint, slab seam, parapet) or ' ' for none.
    ch: char,
    fg: Rgb,
    bg: Rgb,
    /// How much of the street lighting this tile receives (roofs sit above it).
    recv: f32,
    /// Standing water: stronger, glossier reflections and ripples.
    puddle: bool,
}

impl Tile {
    fn new(mat: Mat) -> Tile {
        let (bg, recv) = match mat {
            Mat::Roof => ([0.07, 0.07, 0.16], 0.3),
            Mat::Parapet => ([0.11, 0.11, 0.24], 0.4),
            Mat::Facade => ([0.09, 0.08, 0.18], 0.6),
            Mat::Alley => ([0.08, 0.08, 0.13], 1.0),
            Mat::Sidewalk => ([0.26, 0.26, 0.55], 1.0),
            Mat::Curb => ([0.12, 0.12, 0.18], 1.0),
            Mat::Road => ([0.1, 0.1, 0.14], 1.0),
            Mat::Floor => ([0.45, 0.36, 0.3], 1.0),
        };
        Tile { mat, ch: ' ', fg: [0.0; 3], bg, recv, puddle: false }
    }
    fn mark(mut self, ch: char, fg: Rgb) -> Tile {
        self.ch = ch;
        self.fg = fg;
        self
    }
    fn wet(&self) -> bool {
        matches!(self.mat, Mat::Road | Mat::Alley | Mat::Curb) || self.puddle
    }
}

// ---------------------------------------------------------------- things

enum Shape {
    Point { r: f32 },
    /// A beam along x: dir is +1 or -1.
    Cone { dir: f32, len: f32 },
    /// A reflection smeared down the wet street below a sign.
    Streak { half_w: f32, len: f32 },
}

struct Light {
    x: f32,
    y: f32,
    color: Rgb,
    power: f32,
    shape: Shape,
}

struct Sign {
    x: usize,
    y: usize,
    text: Vec<char>,
    color: Rgb,
    /// Steady hum phase.
    phase: f32,
    /// Prone to dropping out entirely now and then.
    buzzy: bool,
    off_for: f32,
    /// A letter that has died; it sputters back sometimes.
    dead: Option<usize>,
    sputter: f32,
    /// Whether the glow reaches down across the road.
    north: bool,
}

struct Lamp {
    x: usize,
    y: usize,
    color: Rgb,
    broken: bool,
    level: f32,
}

struct Skylight {
    x: usize,
    y: usize,
    color: Rgb,
    on: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum CarKind {
    Sedan,
    Truck,
    Police,
    Bike,
}

struct Car {
    x: f32,
    row: usize,
    dir: f32,
    speed: f32,
    cruise: f32,
    kind: CarKind,
    paint: Rgb,
}

impl Car {
    fn len(&self) -> usize {
        match self.kind {
            CarKind::Sedan | CarKind::Police => 7,
            CarKind::Truck => 11,
            CarKind::Bike => 3,
        }
    }
    /// Leftmost column, front at the right when heading east.
    fn left(&self) -> f32 {
        self.x
    }
    fn front(&self) -> f32 {
        if self.dir > 0.0 {
            self.x + self.len() as f32
        } else {
            self.x
        }
    }
}

struct Walker {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    pause: f32,
    umbrella: Option<Rgb>,
    coat: Rgb,
    /// Inside the lobby: wanders in its box instead of walking the street.
    indoor: bool,
}

struct Drop {
    x: f32,
    y: f32,
    age: u8,
    life: u8,
}

struct Ripple {
    x: usize,
    y: usize,
    age: u8,
}

struct Steam {
    x: f32,
    y: f32,
    age: f32,
}

#[derive(Clone, Copy)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

// ---------------------------------------------------------------- scene

struct Scene {
    w: usize,
    h: usize,
    tiles: Vec<Tile>,
    rng: fastrand::Rng,
    signs: Vec<Sign>,
    lamps: Vec<Lamp>,
    skylights: Vec<Skylight>,
    beacons: Vec<(usize, usize, f32)>,
    vents: Vec<(usize, usize)>,
    doors: Vec<(usize, usize, Rgb)>,
    lobby: Option<Rect>,
    lobby_lights: Vec<(f32, f32)>,
    cars: Vec<Car>,
    walkers: Vec<Walker>,
    drops: Vec<Drop>,
    ripples: Vec<Ripple>,
    steam: Vec<Steam>,
    /// Rows of the street band, top to bottom.
    walk_n: [usize; 2],
    lanes: [usize; 2],
    walk_s: [usize; 2],
    crosswalk: Option<(usize, usize)>,
    t: f32,
    storm: f32,
    storm_target: f32,
    next_storm: f32,
    flash: f32,
    flash_t: f32,
    next_flash: f32,
    next_car: [f32; 2],
    light: Vec<Rgb>,
}

impl Scene {
    fn new(w: usize, h: usize, seed: u64) -> Scene {
        let mut s = Scene {
            w,
            h,
            tiles: vec![Tile::new(Mat::Road); w * h],
            rng: fastrand::Rng::with_seed(seed),
            signs: Vec::new(),
            lamps: Vec::new(),
            skylights: Vec::new(),
            beacons: Vec::new(),
            vents: Vec::new(),
            doors: Vec::new(),
            lobby: None,
            lobby_lights: Vec::new(),
            cars: Vec::new(),
            walkers: Vec::new(),
            drops: Vec::new(),
            ripples: Vec::new(),
            steam: Vec::new(),
            walk_n: [0; 2],
            lanes: [0; 2],
            walk_s: [0; 2],
            crosswalk: None,
            t: 0.0,
            storm: 0.6,
            storm_target: 0.6,
            next_storm: 8.0,
            flash: 0.0,
            flash_t: 99.0,
            next_flash: 0.0,
            next_car: [0.0, 0.5],
            light: vec![[0.0; 3]; w * h],
        };
        s.next_flash = s.rng.f32() * 10.0 + 6.0;
        s.build();
        s.populate();
        s
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }
    fn tile(&self, x: usize, y: usize) -> &Tile {
        &self.tiles[y * self.w + x]
    }
    fn set(&mut self, x: usize, y: usize, t: Tile) {
        if x < self.w && y < self.h {
            let i = self.idx(x, y);
            self.tiles[i] = t;
        }
    }

    /// Lays out the street band in the middle and a block of buildings on
    /// each side of it.
    fn build(&mut self) {
        let (w, h) = (self.w, self.h);
        // Band rows: facade, 2 sidewalk, curb, 2 lane, centre, 2 lane, curb,
        // 2 sidewalk, facade.
        let band = 13usize;
        let top = if h > band { (h - band) * 6 / 10 } else { 0 };
        let r = |k: usize| top + k;
        self.walk_n = [r(1), r(2)];
        self.lanes = [r(4), r(7)];
        self.walk_s = [r(10), r(11)];

        for y in 0..h {
            for x in 0..w {
                let t = if y < r(0) || y > r(12) {
                    Tile::new(Mat::Roof)
                } else if y == r(0) || y == r(12) {
                    Tile::new(Mat::Facade)
                } else if y == r(1) || y == r(2) || y == r(10) || y == r(11) {
                    let mut t = Tile::new(Mat::Sidewalk);
                    if x % 6 == 0 {
                        t = t.mark('▏', [0.16, 0.16, 0.34]);
                    }
                    t
                } else if y == r(3) {
                    Tile::new(Mat::Curb).mark('▀', [0.42, 0.42, 0.6])
                } else if y == r(9) {
                    Tile::new(Mat::Curb).mark('▄', [0.42, 0.42, 0.6])
                } else if y == r(6) {
                    let t = Tile::new(Mat::Road);
                    if (x / 3) % 2 == 0 {
                        t.mark('━', [0.75, 0.6, 0.12])
                    } else {
                        t
                    }
                } else {
                    Tile::new(Mat::Road)
                };
                self.set(x, y, t);
            }
        }

        // A crosswalk somewhere in the middle third.
        if w > 30 {
            let cx = w / 3 + self.rng.usize(0..w / 3);
            for y in r(4)..=r(8) {
                for x in cx..cx + 5 {
                    if x < w && x % 2 == 0 {
                        self.set(x, y, Tile::new(Mat::Road).mark('█', [0.42, 0.42, 0.5]));
                    }
                }
            }
            self.crosswalk = Some((cx, cx + 5));
        }

        // Manholes that steam.
        for _ in 0..(w / 45).max(1) {
            let x = self.rng.usize(0..w);
            let y = if self.rng.bool() { r(5) } else { r(7) };
            if y < self.h && self.tile(x, y).ch == ' ' {
                self.set(x, y, Tile::new(Mat::Road).mark('◘', [0.3, 0.3, 0.38]));
                self.vents.push((x, y));
            }
        }

        // Puddles on the street and pavements.
        let puddles = (w * 13) / 40;
        for _ in 0..puddles {
            let x = self.rng.usize(0..w);
            let y = r(1) + self.rng.usize(0..11);
            let len = self.rng.usize(1..6);
            for dx in 0..len {
                if x + dx < w && y < h && self.tile(x + dx, y).mat != Mat::Curb {
                    let i = self.idx(x + dx, y);
                    self.tiles[i].puddle = true;
                }
            }
        }

        self.build_block(0, r(0), true);
        if r(12) < h {
            self.build_block(r(12), h - 1, false);
        }

        // Street lamps along both pavements, staggered.
        let mut x = self.rng.usize(4..12);
        while x < w {
            let warm = self.rng.f32() < 0.7;
            let color = if warm { [1.0, 0.72, 0.4] } else { [0.7, 0.85, 1.0] };
            let broken = self.rng.f32() < 0.12;
            self.lamps.push(Lamp { x, y: r(2), color, broken, level: 1.0 });
            let x2 = x + 10;
            if x2 < w && r(10) < h {
                self.lamps.push(Lamp { x: x2, y: r(10), color, broken: false, level: 1.0 });
            }
            x += self.rng.usize(18..26);
        }
    }

    /// Fills rows y0..=y1 with buildings and alleys. `north` blocks face
    /// the street at their bottom row (y1), southern ones at their top (y0).
    fn build_block(&mut self, y0: usize, y1: usize, north: bool) {
        let w = self.w;
        let face = if north { y1 } else { y0 };
        let mut x = 0usize;
        let mut lobby_done = false;
        while x < w {
            let bw = self.rng.usize(12..30).min(w - x);
            let b = Rect { x0: x, y0, x1: x + bw - 1, y1 };
            // Parapet outline on the roof part.
            for yy in y0..=y1 {
                for xx in b.x0..=b.x1 {
                    if yy == face {
                        continue;
                    }
                    let edge = xx == b.x0 || xx == b.x1 || yy == y0 || yy == y1;
                    if edge {
                        self.set(xx, yy, Tile::new(Mat::Parapet).mark('░', [0.18, 0.18, 0.4]));
                    }
                }
            }
            let tall = y1.saturating_sub(y0);
            let make_lobby = north && !lobby_done && bw >= 16 && tall >= 6 && self.rng.f32() < 0.5;
            if make_lobby {
                // A glass-roofed lobby cut open to the sky, people inside.
                let l = Rect { x0: b.x0 + 2, y0: y1.saturating_sub(4), x1: b.x1 - 2, y1: y1 - 1 };
                for yy in l.y0..=l.y1 {
                    for xx in l.x0..=l.x1 {
                        let mut t = Tile::new(Mat::Floor);
                        if (xx + yy) % 2 == 0 {
                            t = t.mark('░', [0.5, 0.42, 0.35]);
                        }
                        self.set(xx, yy, t);
                    }
                }
                for xx in l.x0..=l.x1 {
                    self.set(xx, y1, Tile::new(Mat::Facade).mark('─', [0.3, 0.7, 0.8]));
                }
                let mid = (l.x0 + l.x1) / 2;
                self.set(mid, y1, Tile::new(Mat::Sidewalk));
                self.set(mid + 1, y1, Tile::new(Mat::Sidewalk));
                self.lobby = Some(l);
                self.lobby_lights = vec![
                    (l.x0 as f32 + 2.0, (l.y0 + l.y1) as f32 / 2.0),
                    (l.x1 as f32 - 2.0, (l.y0 + l.y1) as f32 / 2.0),
                ];
                lobby_done = true;
            } else {
                // Skylights in a grid, with the odd rooftop unit and beacon.
                let (sy0, sy1) = (y0 + 1, y1.saturating_sub(1));
                let step_x = self.rng.usize(3..5);
                let color = NEON[[2, 6, 1, 2, 6][self.rng.usize(0..5)]];
                let mut yy = sy0 + 1;
                while yy + 1 <= sy1 {
                    let mut xx = b.x0 + 2;
                    while xx + 2 <= b.x1 {
                        if yy != face {
                            let roll = self.rng.f32();
                            if roll < 0.06 {
                                self.set(xx, yy, Tile::new(Mat::Roof).mark('▣', [0.3, 0.3, 0.38]));
                            } else if roll < 0.75 {
                                let on = self.rng.f32() < 0.55;
                                let tint = scale(color, 0.6 + self.rng.f32() * 0.5);
                                self.skylights.push(Skylight { x: xx, y: yy, color: tint, on });
                            }
                        }
                        xx += step_x;
                    }
                    yy += 2;
                }
                if self.rng.f32() < 0.4 && tall > 3 {
                    let bx = self.rng.usize(b.x0 + 1..=b.x1.saturating_sub(1).max(b.x0 + 1));
                    let by = y0 + 1;
                    self.beacons.push((bx, by, self.rng.f32()));
                }
            }

            // Storefront: a sign if it fits, maybe a lit door.
            if !make_lobby || !north {
                let text = SIGNS[self.rng.usize(0..SIGNS.len())];
                let len = text.chars().count();
                if len + 4 <= bw && self.rng.f32() < if north { 0.85 } else { 0.6 } {
                    let sx = b.x0 + 2 + self.rng.usize(0..=(bw - len - 4));
                    let dead = if self.rng.f32() < 0.3 {
                        let i = self.rng.usize(0..len);
                        if text.chars().nth(i) == Some(' ') { None } else { Some(i) }
                    } else {
                        None
                    };
                    self.signs.push(Sign {
                        x: sx,
                        y: face,
                        text: text.chars().collect(),
                        color: NEON[self.rng.usize(0..NEON.len())],
                        phase: self.rng.f32() * 6.28,
                        buzzy: self.rng.f32() < 0.25,
                        off_for: 0.0,
                        dead,
                        sputter: 0.0,
                        north,
                    });
                } else if bw > 6 {
                    let dx = b.x0 + 2 + self.rng.usize(0..bw - 4);
                    let warm = [[1.0, 0.8, 0.5], [0.6, 0.9, 1.0], [1.0, 0.5, 0.7]][self.rng.usize(0..3)];
                    self.doors.push((dx, face, warm));
                }
            }

            x += bw;
            // Alley.
            if x < w {
                let aw = self.rng.usize(1..3).min(w - x);
                for yy in y0..=y1 {
                    for xx in x..x + aw {
                        let mut t = Tile::new(Mat::Alley);
                        if self.rng.f32() < 0.15 {
                            t.puddle = true;
                        }
                        self.set(xx, yy, t);
                    }
                }
                x += aw;
            }
        }
    }

    fn populate(&mut self) {
        // Traffic already under way.
        for lane in 0..2 {
            let mut x = self.rng.f32() * 20.0;
            while x < self.w as f32 {
                self.spawn_car(lane, Some(x));
                x += self.rng.f32() * 40.0 + 30.0;
            }
        }
        let n = (self.w / 9).max(2);
        for _ in 0..n {
            let x = self.rng.f32() * self.w as f32;
            self.spawn_walker(Some(x));
        }
        if let Some(l) = self.lobby {
            for _ in 0..self.rng.usize(3..6) {
                let x = l.x0 as f32 + self.rng.f32() * (l.x1 - l.x0) as f32;
                let y = l.y0 as f32 + self.rng.f32() * (l.y1 - l.y0) as f32;
                let coat = PAINT[self.rng.usize(0..PAINT.len())];
                self.walkers.push(Walker { x, y, vx: 0.0, vy: 0.0, pause: self.rng.f32() * 3.0, umbrella: None, coat, indoor: true });
            }
        }
    }

    fn spawn_car(&mut self, lane: usize, at: Option<f32>) {
        let row = self.lanes[lane];
        if row + 1 >= self.h {
            return;
        }
        let dir = if lane == 0 { -1.0 } else { 1.0 };
        let roll = self.rng.f32();
        let kind = if roll < 0.05 {
            CarKind::Police
        } else if roll < 0.22 {
            CarKind::Truck
        } else if roll < 0.32 {
            CarKind::Bike
        } else {
            CarKind::Sedan
        };
        let cruise = match kind {
            CarKind::Truck => 7.0 + self.rng.f32() * 3.0,
            CarKind::Bike => 13.0 + self.rng.f32() * 5.0,
            CarKind::Police => 16.0 + self.rng.f32() * 3.0,
            CarKind::Sedan => 9.0 + self.rng.f32() * 6.0,
        };
        let mut car = Car { x: 0.0, row, dir, speed: cruise, cruise, kind, paint: PAINT[self.rng.usize(0..PAINT.len())] };
        if kind == CarKind::Police {
            car.paint = [0.85, 0.85, 0.9];
        }
        let len = car.len() as f32;
        car.x = match at {
            Some(x) => x,
            None if dir > 0.0 => -len - 1.0,
            None => self.w as f32 + 1.0,
        };
        // Do not drop a car onto another one.
        let clear = self.cars.iter().all(|c| c.row != row || (c.x - car.x).abs() > len.max(c.len() as f32) + 3.0);
        if clear {
            self.cars.push(car);
        }
    }

    fn spawn_walker(&mut self, at: Option<f32>) {
        let rows = [self.walk_n[0], self.walk_n[1], self.walk_s[0], self.walk_s[1]];
        let y = rows[self.rng.usize(0..4)];
        if y >= self.h {
            return;
        }
        let dir = if self.rng.bool() { 1.0 } else { -1.0 };
        let x = at.unwrap_or(if dir > 0.0 { -1.0 } else { self.w as f32 });
        let umbrella = if self.rng.f32() < 0.6 { Some(UMBRELLAS[self.rng.usize(0..UMBRELLAS.len())]) } else { None };
        let coat = PAINT[self.rng.usize(0..PAINT.len())];
        let vx = dir * (1.6 + self.rng.f32() * 1.8);
        self.walkers.push(Walker { x, y: y as f32, vx, vy: 0.0, pause: 0.0, umbrella, coat, indoor: false });
    }

    // ------------------------------------------------------------ update

    fn update(&mut self, dt: f32) {
        self.t += dt;
        let w = self.w as f32;

        // Storm strength drifts between drizzle and downpour.
        self.next_storm -= dt;
        if self.next_storm <= 0.0 {
            self.storm_target = 0.25 + self.rng.f32() * 0.9;
            self.next_storm = 6.0 + self.rng.f32() * 12.0;
        }
        self.storm += (self.storm_target - self.storm) * dt * 0.3;

        // Lightning: a bright double flash that fades.
        self.next_flash -= dt;
        if self.next_flash <= 0.0 {
            self.flash_t = 0.0;
            log::debug!("lightning at {:.1}s", self.t);
            self.next_flash = 12.0 + self.rng.f32() * 25.0;
        }
        self.flash_t += dt;
        let ft = self.flash_t;
        self.flash = if ft < 0.07 {
            1.0
        } else if ft < 0.14 {
            0.15
        } else if ft < 0.22 {
            0.8
        } else {
            0.8 * (-(ft - 0.22) * 4.0).exp()
        };

        // Signs.
        for s in &mut self.signs {
            if s.off_for > 0.0 {
                s.off_for -= dt;
            } else if s.buzzy && self.rng.f32() < dt * 0.6 {
                s.off_for = 0.03 + self.rng.f32() * 0.25;
            }
            if s.dead.is_some() {
                s.sputter = if self.rng.f32() < dt * 1.5 { 0.05 + self.rng.f32() * 0.15 } else { (s.sputter - dt).max(0.0) };
            }
        }
        for l in &mut self.lamps {
            l.level = if l.broken {
                if self.rng.f32() < 0.12 { self.rng.f32() * 0.3 } else { 1.0 }
            } else {
                1.0
            };
        }
        for s in &mut self.skylights {
            if self.rng.f32() < dt * 0.02 {
                s.on = !s.on;
            }
        }

        // Traffic: keep a gap to the car ahead, otherwise cruise.
        for lane in 0..2 {
            self.next_car[lane] -= dt;
            if self.next_car[lane] <= 0.0 {
                self.spawn_car(lane, None);
                self.next_car[lane] = 3.0 + self.rng.f32() * 7.0;
            }
        }
        let crosswalk_busy = self.crosswalk.map(|(a, b)| {
            self.walkers.iter().any(|p| !p.indoor && p.vy != 0.0 && p.x >= a as f32 - 1.0 && p.x <= b as f32 + 1.0)
        });
        let snapshot: Vec<(usize, f32, f32, f32)> = self.cars.iter().map(|c| (c.row, c.left(), c.len() as f32, c.speed)).collect();
        for (i, c) in self.cars.iter_mut().enumerate() {
            let mut target = c.cruise;
            for (j, &(row, left, len, speed)) in snapshot.iter().enumerate() {
                if i == j || row != c.row {
                    continue;
                }
                let gap = if c.dir > 0.0 { left - c.front() } else { c.front() - (left + len) };
                if gap > -1.0 && gap < 6.0 {
                    target = target.min(speed * (gap / 6.0).max(0.0));
                }
            }
            if let (Some(true), Some((a, b))) = (crosswalk_busy, self.crosswalk) {
                let stop_at = if c.dir > 0.0 { a as f32 - 1.0 } else { b as f32 + 1.0 };
                let gap = (stop_at - c.front()) * c.dir;
                if gap > -0.5 && gap < 10.0 {
                    target = target.min(c.cruise * (gap / 10.0).max(0.0));
                }
            }
            let rate = if target < c.speed { 25.0 } else { 6.0 };
            c.speed += (target - c.speed).clamp(-rate * dt, rate * dt);
            c.x += c.dir * c.speed * dt;
        }
        self.cars.retain(|c| c.x > -15.0 && c.x < w + 15.0);

        // People.
        let lobby = self.lobby;
        let walk_rows = [self.walk_n, self.walk_s];
        let cross = self.crosswalk;
        let (lane_top, lane_bottom) = (self.lanes[0] as f32 - 1.0, self.lanes[1] as f32 + 2.0);
        for p in &mut self.walkers {
            if p.pause > 0.0 {
                p.pause -= dt;
                continue;
            }
            if p.indoor {
                let l = lobby.unwrap();
                if p.vx == 0.0 && p.vy == 0.0 {
                    p.vx = (self.rng.f32() - 0.5) * 3.0;
                    p.vy = (self.rng.f32() - 0.5) * 1.2;
                }
                p.x += p.vx * dt;
                p.y += p.vy * dt;
                if p.x < l.x0 as f32 || p.x > l.x1 as f32 || p.y < l.y0 as f32 || p.y > l.y1 as f32 + 0.9 {
                    p.x = p.x.clamp(l.x0 as f32, l.x1 as f32);
                    p.y = p.y.clamp(l.y0 as f32, l.y1 as f32);
                    p.vx = 0.0;
                    p.vy = 0.0;
                }
                if self.rng.f32() < dt * 0.4 {
                    p.vx = 0.0;
                    p.vy = 0.0;
                    p.pause = self.rng.f32() * 4.0;
                }
                continue;
            }
            if p.vy != 0.0 {
                // Crossing the road.
                p.y += p.vy * dt;
                let row = p.y.round() as usize;
                let dest_n = walk_rows[0][1] as f32;
                let dest_s = walk_rows[1][0] as f32;
                if (p.vy < 0.0 && p.y <= dest_n) || (p.vy > 0.0 && p.y >= dest_s) {
                    p.y = row as f32;
                    p.vy = 0.0;
                    if p.vx == 0.0 {
                        p.vx = if self.rng.bool() { 2.0 } else { -2.0 };
                    }
                }
                continue;
            }
            p.x += p.vx * dt;
            if self.rng.f32() < dt * 0.05 {
                p.pause = 1.0 + self.rng.f32() * 4.0;
            }
            if let Some((a, b)) = cross {
                let mid = (a + b) as f32 / 2.0;
                let at_edge = p.y as usize == walk_rows[0][1] || p.y as usize == walk_rows[1][0];
                if at_edge && (p.x - mid).abs() < 0.5 && self.rng.f32() < 0.35 {
                    p.x = mid.round();
                    p.vy = if p.y < lane_top { 3.0 } else { -3.0 };
                    let _ = lane_bottom;
                    p.pause = 0.6;
                }
            }
        }
        self.walkers.retain(|p| p.indoor || (p.x > -2.0 && p.x < w + 2.0));
        let outdoor = self.walkers.iter().filter(|p| !p.indoor).count();
        if outdoor < (self.w / 9).max(2) && self.rng.f32() < dt * 1.5 {
            self.spawn_walker(None);
        }

        // Rain.
        let area = (self.w * self.h) as f32;
        let mut n = area * self.storm * 0.006;
        while n > 0.0 {
            if n >= 1.0 || self.rng.f32() < n {
                let life = self.rng.u8(2..5);
                self.drops.push(Drop { x: self.rng.f32() * (w + 4.0), y: self.rng.f32() * self.h as f32 - 1.0, age: 0, life });
            }
            n -= 1.0;
        }
        let mut landed = Vec::new();
        for d in &mut self.drops {
            d.age += 1;
            d.x -= 0.9;
            d.y += 0.45;
            if d.age >= d.life {
                landed.push((d.x, d.y));
            }
        }
        self.drops.retain(|d| d.age < d.life);
        for (x, y) in landed {
            if x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h && self.rng.f32() < 0.5 {
                self.ripples.push(Ripple { x: x as usize, y: y as usize, age: 0 });
            }
        }
        for r in &mut self.ripples {
            r.age += 1;
        }
        self.ripples.retain(|r| r.age < 5);

        // Steam from the manholes, pushed along by the wind.
        for &(x, y) in &self.vents {
            if self.rng.f32() < dt * 4.0 {
                self.steam.push(Steam { x: x as f32, y: y as f32, age: 0.0 });
            }
        }
        for s in &mut self.steam {
            s.age += dt;
            s.x -= dt * (1.5 + self.rng.f32());
            s.y -= dt * 0.4 * self.rng.f32();
        }
        self.steam.retain(|s| s.age < 2.5);
    }

    // ------------------------------------------------------------ lighting

    fn lights(&self) -> Vec<Light> {
        let mut out = Vec::new();
        for s in &self.signs {
            let k = self.sign_level(s);
            if k <= 0.0 {
                continue;
            }
            let len = s.text.len() as f32;
            let cx = s.x as f32 + len / 2.0;
            let cy = if s.north { s.y as f32 + 1.0 } else { s.y as f32 - 1.0 };
            out.push(Light { x: cx, y: cy, color: s.color, power: 1.1 * k, shape: Shape::Point { r: len * 0.7 + 7.0 } });
            if s.north {
                out.push(Light { x: cx, y: s.y as f32 + 1.0, color: s.color, power: 0.9 * k, shape: Shape::Streak { half_w: len / 2.0 + 1.0, len: 11.0 } });
            }
        }
        for l in &self.lamps {
            out.push(Light { x: l.x as f32, y: l.y as f32, color: l.color, power: 0.75 * l.level, shape: Shape::Point { r: 10.0 } });
        }
        for &(x, y, c) in &self.doors {
            out.push(Light { x: x as f32, y: y as f32, color: c, power: 0.6, shape: Shape::Point { r: 5.0 } });
        }
        for &(x, y) in &self.lobby_lights {
            out.push(Light { x, y, color: [1.0, 0.85, 0.6], power: 0.9, shape: Shape::Point { r: 9.0 } });
        }
        for c in &self.cars {
            let mid = c.row as f32 + 0.5;
            let head = [0.75, 0.95, 1.0];
            match c.kind {
                CarKind::Bike => {
                    out.push(Light { x: c.front(), y: mid, color: head, power: 0.5, shape: Shape::Cone { dir: c.dir, len: 8.0 } });
                }
                _ => {
                    out.push(Light { x: c.front(), y: mid, color: head, power: 0.65, shape: Shape::Cone { dir: c.dir, len: 13.0 } });
                }
            }
            let back = if c.dir > 0.0 { c.x } else { c.x + c.len() as f32 };
            out.push(Light { x: back, y: mid, color: [1.0, 0.08, 0.1], power: 0.55, shape: Shape::Point { r: 4.5 } });
            if c.kind == CarKind::Police {
                let red = ((self.t * 6.0) as i32) % 2 == 0;
                let color = if red { [1.0, 0.1, 0.15] } else { [0.15, 0.3, 1.0] };
                out.push(Light { x: c.x + c.len() as f32 / 2.0, y: mid, color, power: 1.0, shape: Shape::Point { r: 11.0 } });
            }
        }
        out
    }

    fn sign_level(&self, s: &Sign) -> f32 {
        if s.off_for > 0.0 {
            return 0.0;
        }
        0.9 + 0.1 * (self.t * 7.0 + s.phase).sin()
    }

    fn light_up(&mut self) {
        let base = add(AMBIENT, scale(FLASH, self.flash));
        for v in &mut self.light {
            *v = [0.0; 3];
        }
        let (w, h) = (self.w as i32, self.h as i32);
        let lights = self.lights();
        for l in &lights {
            let (x0, x1, y0, y1) = match l.shape {
                Shape::Point { r } => (l.x - r, l.x + r, l.y - r / 2.0, l.y + r / 2.0),
                Shape::Cone { dir, len } => {
                    let (a, b) = if dir > 0.0 { (l.x, l.x + len) } else { (l.x - len, l.x) };
                    (a, b, l.y - len / 4.0, l.y + len / 4.0)
                }
                Shape::Streak { half_w, len } => (l.x - half_w - 1.0, l.x + half_w + 1.0, l.y, l.y + len),
            };
            let (xa, xb) = ((x0.floor() as i32).max(0), (x1.ceil() as i32).min(w - 1));
            let (ya, yb) = ((y0.floor() as i32).max(0), (y1.ceil() as i32).min(h - 1));
            for y in ya..=yb {
                for x in xa..=xb {
                    let dx = x as f32 + 0.5 - l.x;
                    let dy = (y as f32 + 0.5 - l.y) * 2.0;
                    let i = (y * w + x) as usize;
                    let tile = &self.tiles[i];
                    let f = match l.shape {
                        Shape::Point { r } => {
                            let d = (dx * dx + dy * dy).sqrt();
                            if d >= r { 0.0 } else { (1.0 - d / r).powi(2) }
                        }
                        Shape::Cone { dir, len } => {
                            let along = dx * dir;
                            let spread = 1.0 + along * 0.35;
                            if along < 0.0 || along > len || dy.abs() > spread * 2.0 {
                                0.0
                            } else {
                                (1.0 - along / len).powf(1.3) * (1.0 - dy.abs() / (spread * 2.0 + 0.01)).max(0.0).sqrt()
                            }
                        }
                        Shape::Streak { half_w, len } => {
                            if !tile.wet() || dy < 0.0 {
                                0.0
                            } else {
                                let side = (1.0 - dx.abs() / (half_w + 1.0)).max(0.0);
                                let down = 1.0 - (dy / 2.0) / len;
                                // A shimmer as the rain breaks up the reflection.
                                let wave = 0.75 + 0.25 * ((y as f32) * 1.7 + self.t * 9.0 + x as f32 * 0.3).sin();
                                side * down.max(0.0) * wave
                            }
                        }
                    };
                    if f > 0.0 {
                        let k = f * l.power * tile.recv;
                        let v = &mut self.light[i];
                        *v = add(*v, scale(l.color, k));
                    }
                }
            }
        }
        // Keep the ambient separate so the street can tell pools of light from
        // the general gloom.
        for v in &mut self.light {
            *v = add(*v, [0.0; 3]);
        }
        let _ = base;
    }

    // ------------------------------------------------------------ drawing

    fn render(&mut self, out: &mut Frame) {
        self.light_up();
        let ambient = add(AMBIENT, scale(FLASH, self.flash));
        let (w, h) = (self.w, self.h);

        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let t = self.tiles[i];
                let l = self.light[i];
                let lit = add(ambient, l);
                let wet = if t.puddle { 1.6 } else if t.wet() { 1.0 } else { 0.0 };
                let bg = add(mul(t.bg, lit), scale(l, 0.12 * wet));
                let (ch, fg) = if t.ch != ' ' {
                    (t.ch, mul(t.fg, add(lit, scale(l, 0.5))))
                } else if wet > 0.0 {
                    // Pools of light on wet ground, dithered.
                    let k = lum(l) * wet;
                    let ch = if k < 0.12 {
                        ' '
                    } else if k < 0.3 {
                        '░'
                    } else if k < 0.6 {
                        '▒'
                    } else {
                        '▓'
                    };
                    (ch, scale(l, 0.55))
                } else {
                    (' ', [0.0; 3])
                };
                out.put(x, y, ch, fg, bg);
            }
        }

        // Skylights and rooftop beacons glow on their own.
        for s in &self.skylights {
            let c = if s.on { s.color } else { add(scale(s.color, 0.12), scale(ambient, 0.1)) };
            let bg = mul(self.tiles[self.idx(s.x, s.y)].bg, ambient);
            out.put(s.x, s.y, '■', c, bg);
        }
        for &(x, y, ph) in &self.beacons {
            let on = ((self.t * 0.8 + ph).fract()) < 0.15;
            let c = if on { [1.0, 0.15, 0.1] } else { [0.25, 0.05, 0.05] };
            out.fg(x, y, '•', c);
        }

        // Signs and doors on the facades.
        for s in &self.signs {
            let k = self.sign_level(s);
            for (j, &ch) in s.text.iter().enumerate() {
                let dead = s.dead == Some(j) && s.sputter <= 0.0;
                let c = if k == 0.0 || dead {
                    scale(s.color, 0.12)
                } else {
                    scale([s.color[0].max(0.35), s.color[1].max(0.35), s.color[2].max(0.35)], k * 1.1)
                };
                out.fg(s.x + j, s.y, ch, c);
            }
        }
        for &(x, y, c) in &self.doors {
            out.put(x, y, '▒', scale(c, 0.9), scale(c, 0.35));
        }
        for &(x, y) in &self.lobby_lights {
            out.fg(x as usize, y as usize, '◦', [1.0, 0.9, 0.7]);
        }

        // Lamps.
        for l in &self.lamps {
            out.fg(l.x, l.y, '◙', scale(l.color, 0.3 + 0.8 * l.level));
        }

        // Ripples on the ground.
        for r in &self.ripples {
            let i = self.idx(r.x, r.y);
            let t = self.tiles[i];
            if matches!(t.mat, Mat::Roof | Mat::Parapet | Mat::Facade) {
                continue;
            }
            let ch = if t.puddle { ['∘', 'o', 'O', '◌', ' '][r.age as usize % 5] } else { ['·', '∙', ' ', ' ', ' '][r.age as usize % 5] };
            if ch != ' ' {
                let c = add([0.25, 0.3, 0.5], scale(self.light[i], 0.9));
                out.fg(r.x, r.y, ch, add(c, scale(FLASH, self.flash * 0.6)));
            }
        }

        // Steam.
        for s in &self.steam {
            let (x, y) = (s.x.round() as i32, s.y.round() as i32);
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                let k = (1.0 - s.age / 2.5) * 0.6;
                let i = self.idx(x as usize, y as usize);
                let c = scale(add([0.5, 0.5, 0.6], self.light[i]), k);
                out.fg(x as usize, y as usize, if k > 0.35 { '▒' } else { '░' }, c);
            }
        }

        // People.
        for p in &self.walkers {
            let (x, y) = (p.x.round() as i32, p.y.round() as i32);
            if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                continue;
            }
            let i = self.idx(x as usize, y as usize);
            let lit = add(ambient, scale(self.light[i], 1.3));
            match p.umbrella {
                Some(u) => out.fg(x as usize, y as usize, '●', mul(u, add(lit, [0.25; 3]))),
                None => out.fg(x as usize, y as usize, '☻', mul(p.coat, add(lit, [0.35; 3]))),
            }
        }

        // Cars.
        for c in &self.cars {
            self.draw_car(out, c, ambient);
        }

        // Rain over everything.
        for d in &self.drops {
            let (x, y) = (d.x.round() as i32, d.y.round() as i32);
            if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                continue;
            }
            let i = self.idx(x as usize, y as usize);
            let c = add(add([0.2, 0.24, 0.45], scale(self.light[i], 0.85)), scale(FLASH, self.flash * 0.7));
            out.fg(x as usize, y as usize, '/', c);
        }
    }

    fn draw_car(&self, out: &mut Frame, c: &Car, ambient: Rgb) {
        let len = c.len();
        let x0 = c.x.round() as i32;
        let east = c.dir > 0.0;
        let tail = [1.0, 0.1, 0.12];
        let head = [1.0, 1.0, 0.85];
        let glass = [0.06, 0.09, 0.16];
        let rows: &[usize] = if c.kind == CarKind::Bike { &[0] } else { &[0, 1] };
        for &dy in rows {
            let y = c.row + dy + if c.kind == CarKind::Bike { 1 } else { 0 };
            if y >= self.h {
                continue;
            }
            for k in 0..len {
                let x = x0 + k as i32;
                if x < 0 || x as usize >= self.w {
                    continue;
                }
                let xu = x as usize;
                // k counted from the back of the car.
                let from_back = if east { k } else { len - 1 - k };
                let lit = add(scale(ambient, 1.4), scale(self.light[self.idx(xu, y)], 0.6));
                let body = mul(c.paint, lit);
                let (back_half, front_half) = if east { ('▌', '▐') } else { ('▐', '▌') };
                let cell = match c.kind {
                    CarKind::Bike => match from_back {
                        0 => (back_half, tail, [0.0; 3]),
                        1 => ('▬', body, [0.0; 3]),
                        _ => (front_half, head, [0.0; 3]),
                    },
                    CarKind::Truck => match from_back {
                        0 => (back_half, tail, mul([0.55, 0.55, 0.6], lit)),
                        1..=6 => (if from_back % 2 == 0 { '│' } else { ' ' }, mul([0.4, 0.4, 0.45], lit), mul([0.55, 0.55, 0.6], lit)),
                        7 => (' ', [0.0; 3], [0.02, 0.02, 0.03]),
                        8 => (' ', [0.0; 3], body),
                        9 => (front_half, glass, body),
                        _ => (front_half, head, body),
                    },
                    CarKind::Sedan | CarKind::Police => match from_back {
                        0 => (back_half, tail, body),
                        1 => (front_half, glass, body),
                        2 | 3 => {
                            if c.kind == CarKind::Police && from_back == 3 {
                                let red = ((self.t * 6.0) as i32) % 2 == 0;
                                let bar = if (dy == 0) == red { [1.0, 0.1, 0.15] } else { [0.2, 0.35, 1.0] };
                                ('▀', bar, scale(body, 0.75))
                            } else {
                                (' ', [0.0; 3], scale(body, 0.75))
                            }
                        }
                        4 => (back_half, glass, body),
                        5 => (' ', [0.0; 3], body),
                        _ => (front_half, head, body),
                    },
                };
                out.put(xu, y, cell.0, cell.1, cell.2);
            }
        }
    }
}

// ---------------------------------------------------------------- output

#[derive(Clone, Copy, PartialEq)]
struct Cell {
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
}

struct Frame {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
}

fn to8(c: Rgb) -> [u8; 3] {
    let q = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 1.15) * 255.0) as u8;
    [q(c[0]), q(c[1]), q(c[2])]
}

impl Frame {
    fn new(w: usize, h: usize) -> Frame {
        Frame { w, h, cells: vec![Cell { ch: ' ', fg: [0; 3], bg: [0; 3] }; w * h] }
    }
    fn put(&mut self, x: usize, y: usize, ch: char, fg: Rgb, bg: Rgb) {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x] = Cell { ch, fg: to8(fg), bg: to8(bg) };
        }
    }
    /// Draws a glyph over whatever background is already there.
    fn fg(&mut self, x: usize, y: usize, ch: char, fg: Rgb) {
        if x < self.w && y < self.h {
            let c = &mut self.cells[y * self.w + x];
            c.ch = ch;
            c.fg = to8(fg);
        }
    }
}

struct Screen {
    out: BufWriter<io::Stdout>,
    prev: Option<Frame>,
    truecolor: bool,
}

fn ansi256(c: [u8; 3]) -> u8 {
    let [r, g, b] = c;
    if r.abs_diff(g) < 10 && g.abs_diff(b) < 10 {
        let v = (r as u16 + g as u16 + b as u16) / 3;
        if v < 8 {
            return 16;
        }
        if v > 238 {
            return 231;
        }
        return 232 + ((v - 8) / 10).min(23) as u8;
    }
    let s = |v: u8| if v < 48 { 0 } else if v < 115 { 1 } else { (v - 35) / 40 };
    16 + 36 * s(r) + 6 * s(g) + s(b)
}

impl Screen {
    fn color(&self, c: [u8; 3]) -> Color {
        if self.truecolor {
            Color::Rgb { r: c[0], g: c[1], b: c[2] }
        } else {
            Color::AnsiValue(ansi256(c))
        }
    }

    fn draw(&mut self, f: Frame) -> io::Result<()> {
        let mut cur_fg = None;
        let mut cur_bg = None;
        let mut at: Option<(usize, usize)> = None;
        for y in 0..f.h {
            for x in 0..f.w {
                let c = f.cells[y * f.w + x];
                if let Some(p) = &self.prev {
                    if p.cells[y * f.w + x] == c {
                        continue;
                    }
                }
                if at != Some((x, y)) {
                    queue!(self.out, cursor::MoveTo(x as u16, y as u16))?;
                }
                if cur_bg != Some(c.bg) {
                    let col = self.color(c.bg);
                    queue!(self.out, SetBackgroundColor(col))?;
                    cur_bg = Some(c.bg);
                }
                if c.ch != ' ' && cur_fg != Some(c.fg) {
                    let col = self.color(c.fg);
                    queue!(self.out, SetForegroundColor(col))?;
                    cur_fg = Some(c.fg);
                }
                queue!(self.out, Print(c.ch))?;
                at = Some((x + 1, y));
            }
        }
        self.out.flush()?;
        self.prev = Some(f);
        Ok(())
    }
}

// ---------------------------------------------------------------- main

fn usage() {
    println!("neon-rain: a rainy neon street at night, seen from above\n");
    println!("usage: neon-rain [-s SEED]\n");
    println!("  -s, --seed N   lay out the same street every time");
    println!("  -h, --help     this text\n");
    println!("Any key quits.");
}

fn main() -> io::Result<()> {
    let mut seed: Option<u64> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-s" | "--seed" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) => seed = Some(v),
                None => {
                    eprintln!("neon-rain: --seed needs a number");
                    std::process::exit(2);
                }
            },
            "-h" | "--help" => {
                usage();
                return Ok(());
            }
            other => {
                eprintln!("neon-rain: unknown option {other}");
                usage();
                std::process::exit(2);
            }
        }
    }
    let seed = seed.unwrap_or_else(|| fastrand::u64(..));
    // Only writes when RUST_LOG is set: RUST_LOG=debug neon-rain 2>neon-rain.log
    env_logger::init();

    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop))?;
    }

    let truecolor = std::env::var("COLORTERM").map(|v| v.contains("truecolor") || v.contains("24bit")).unwrap_or(false)
        || std::env::var_os("WT_SESSION").is_some()
        || cfg!(windows);

    // Give the terminal back even if something panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        let _ = crossterm::execute!(out, crossterm::style::ResetColor, cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
        hook(info);
    }));

    terminal::enable_raw_mode()?;
    let mut screen = Screen { out: BufWriter::with_capacity(1 << 16, io::stdout()), prev: None, truecolor };
    queue!(screen.out, terminal::EnterAlternateScreen, cursor::Hide, terminal::Clear(terminal::ClearType::All))?;
    screen.out.flush()?;

    let result = run(&mut screen, seed, &stop);

    queue!(screen.out, crossterm::style::ResetColor, cursor::Show, terminal::LeaveAlternateScreen)?;
    screen.out.flush()?;
    terminal::disable_raw_mode()?;
    result
}

fn run(screen: &mut Screen, seed: u64, stop: &AtomicBool) -> io::Result<()> {
    let (w, h) = terminal::size()?;
    log::info!("start {w}x{h} seed {seed} truecolor {}", screen.truecolor);
    let mut scene = Scene::new(w as usize, h as usize, seed);
    let mut next_snapshot = 1.0f32;
    let frame_time = Duration::from_secs_f32(1.0 / FPS);
    let mut last = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            log::info!("signal, quitting");
            return Ok(());
        }
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;
        scene.update(dt);
        if scene.t >= next_snapshot {
            next_snapshot = scene.t + 1.0;
            log::debug!(
                "t {:.1} cars {} walkers {} drops {} storm {:.2} flash {:.2}",
                scene.t, scene.cars.len(), scene.walkers.len(), scene.drops.len(), scene.storm, scene.flash
            );
        }
        let mut f = Frame::new(scene.w, scene.h);
        scene.render(&mut f);
        screen.draw(f)?;

        let deadline = now + frame_time;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if !event::poll(left)? {
                break;
            }
            let ev = event::read()?;
            log::debug!("event {ev:?}");
            match ev {
                Event::Key(k) if k.kind == KeyEventKind::Press => return Ok(()),
                Event::Resize(nw, nh) => {
                    next_snapshot = 1.0;
                    scene = Scene::new(nw as usize, nh as usize, seed);
                    screen.prev = None;
                    queue!(screen.out, crossterm::style::ResetColor, terminal::Clear(terminal::ClearType::All))?;
                }
                _ => {}
            }
        }
    }
}
