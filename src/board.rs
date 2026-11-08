//! Board geometry and text formatting for the round view. Pure Rust, no GUI types.
//!
//! The Slint side only draws what this module computes:
//!
//! * [`screen_vector`] maps a compass bearing to a screen direction (north = up).
//! * [`layout_board`] turns a canvas size and the five hints into arrow shapes (fixed length,
//!   exact bearing) and label boxes that never overlap an arrow, the dial, the compass rose,
//!   each other or the canvas edge.
//! * [`format_hint_label`] / [`format_distance`] produce the French texts (`Lyon · 392 km`).
//!
//! ## Label placement
//!
//! Every arrow ends on the circle of radius `tip_radius`. A label is anchored on the circle just
//! outside it, at an *anchor angle* that starts at the arrow's bearing: the box touches the
//! supporting line of that circle point, on the outer side, so a label can never cover any arrow
//! (all arrows end inside the circle). Where two bearings are close (down to 15° for
//! Porto-Vecchio) the anchor angles are spread apart by a small deterministic coordinate descent
//! until no label overlaps another one, the compass rose or the canvas edge. Arrow *directions*
//! are never touched; only label anchors move, and they stay within [`MAX_ANCHOR_SHIFT_DEG`]
//! of their arrow.

use crate::game::{Difficulty, Hint};
use crate::text::{display_text_width, label_text_width};

/// Font size of the arrow labels, in logical pixels (matches `ui/board.slint`).
pub const LABEL_FONT_PX: f64 = 14.0;
/// Height of a label pill.
pub const LABEL_HEIGHT: f64 = 28.0;
/// Horizontal padding inside a label pill.
pub const LABEL_PAD_X: f64 = 11.0;
/// Space between the arrow tips' circle and the nearest label edge.
pub const LABEL_GAP: f64 = 8.0;
/// Minimum space between two labels.
pub const LABEL_SPACING: f64 = 6.0;
/// Minimum space between a label and the canvas edge.
pub const EDGE_MARGIN: f64 = 12.0;
/// Side of the square reserved for the compass rose in the bottom-right corner.
pub const COMPASS_SIZE: f64 = 72.0;
/// Arrowhead length and half-width.
pub const HEAD_LEN: f64 = 17.0;
pub const HEAD_HALF_WIDTH: f64 = 8.0;
/// Largest distance a label anchor may drift from its arrow's bearing, in degrees.
pub const MAX_ANCHOR_SHIFT_DEG: f64 = 75.0;
/// Clearance kept between a label and an arrow (shaft or head outline).
const ARROW_CLEARANCE: f64 = 5.0;

/// Size of the unit square the dial tick path is drawn in (centre at 100, 100).
pub const DIAL_UNIT: f64 = 200.0;
/// Radius of the dial's body (the filled disc) relative to its outer radius (the tick ring).
pub const DIAL_BODY_RATIO: f64 = 0.78;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn offset(self, direction: Point, distance: f64) -> Point {
        Point::new(
            self.x + direction.x * distance,
            self.y + direction.y * distance,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn from_center(center: Point, w: f64, h: f64) -> Self {
        Self::new(center.x - w / 2.0, center.y - h / 2.0, w, h)
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Overlap area of the two rectangles once each is grown by `gap / 2` (0 when apart).
    pub fn overlap_area(&self, other: &Rect, gap: f64) -> f64 {
        let px = (self.w + other.w) / 2.0 + gap - (self.center().x - other.center().x).abs();
        let py = (self.h + other.h) / 2.0 + gap - (self.center().y - other.center().y).abs();
        if px > 0.0 && py > 0.0 { px * py } else { 0.0 }
    }

    /// Distance from `p` to the rectangle (0 when inside).
    pub fn distance_to_point(&self, p: Point) -> f64 {
        let dx = (self.x - p.x).max(0.0).max(p.x - self.right());
        let dy = (self.y - p.y).max(0.0).max(p.y - self.bottom());
        dx.hypot(dy)
    }

    /// How far the rectangle sticks out of `bounds` (sum of the four overshoots).
    pub fn overshoot(&self, bounds: &Rect) -> f64 {
        (bounds.x - self.x).max(0.0)
            + (self.right() - bounds.right()).max(0.0)
            + (bounds.y - self.y).max(0.0)
            + (self.bottom() - bounds.bottom()).max(0.0)
    }
}

/// Screen direction of a compass bearing: north is up, east is right
/// (`dx = sin θ`, `dy = −cos θ`, screen y grows downwards).
pub fn screen_vector(bearing_deg: f64) -> Point {
    let theta = bearing_deg.to_radians();
    Point::new(theta.sin(), -theta.cos())
}

// ---------------------------------------------------------------------------------------------
// Texts
// ---------------------------------------------------------------------------------------------

/// Kilometres rounded to the nearest integer, grouped by thousands with a space
/// (`392`, `1 047`).
pub fn format_km(distance_km: f64) -> String {
    let km = distance_km.round().max(0.0) as u64;
    let digits = km.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(' ');
        }
        grouped.push(ch);
    }
    grouped
}

/// `312 km`, used in the guess history.
pub fn format_distance(distance_km: f64) -> String {
    format!("{} km", format_km(distance_km))
}

/// Text of an arrow label: the city name, plus ` · NNN km` only when the difficulty shows
/// distances (`Lyon · 392 km` in Facile, `Lyon` in Difficile).
pub fn format_hint_label(hint: &Hint, difficulty: Difficulty) -> String {
    if difficulty.shows_distances() {
        format!("{} · {}", hint.city.name, format_distance(hint.distance_km))
    } else {
        hint.city.name.to_owned()
    }
}

/// Text of an arrow label from what the server sent: the city name, plus ` · NNN km` iff the
/// hint carries a distance (only Facile rooms do; a Difficile room never sends one).
pub fn format_dto_label(city: &str, distance_km: Option<u32>) -> String {
    match distance_km {
        Some(km) => format!("{city} · {}", format_distance(f64::from(km))),
        None => city.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------------------------

/// One arrow: a shaft from the dial outwards ending in a filled head. All arrows of a board have
/// the same length; it never encodes the distance.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrow {
    pub bearing_deg: f64,
    pub start: Point,
    pub shaft_end: Point,
    pub tip: Point,
    pub head: [Point; 3],
}

impl Arrow {
    fn new(center: Point, bearing_deg: f64, start_radius: f64, tip_radius: f64) -> Self {
        let dir = screen_vector(bearing_deg);
        let normal = Point::new(-dir.y, dir.x);
        let start = center.offset(dir, start_radius);
        let tip = center.offset(dir, tip_radius);
        let base = tip.offset(dir, -HEAD_LEN);
        Arrow {
            bearing_deg,
            start,
            // The shaft runs a little under the head so the join has no gap.
            shaft_end: base.offset(dir, 3.0),
            tip,
            head: [
                tip,
                base.offset(normal, HEAD_HALF_WIDTH),
                base.offset(normal, -HEAD_HALF_WIDTH),
            ],
        }
    }

    /// The segments that make up the drawn arrow: the shaft and the three edges of the head.
    pub fn outline(&self) -> [(Point, Point); 4] {
        [
            (self.start, self.shaft_end),
            (self.head[0], self.head[1]),
            (self.head[1], self.head[2]),
            (self.head[2], self.head[0]),
        ]
    }

    /// SVG path of the shaft, in canvas coordinates.
    pub fn shaft_commands(&self) -> String {
        format!(
            "M {:.2} {:.2} L {:.2} {:.2}",
            self.start.x, self.start.y, self.shaft_end.x, self.shaft_end.y
        )
    }

    /// SVG path of the (closed, filled) arrowhead, in canvas coordinates.
    pub fn head_commands(&self) -> String {
        format!(
            "M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} Z",
            self.head[0].x,
            self.head[0].y,
            self.head[1].x,
            self.head[1].y,
            self.head[2].x,
            self.head[2].y
        )
    }
}

/// One label pill next to an arrow tip.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    pub rect: Rect,
    /// Where the arrow it belongs to ends.
    pub tip: Point,
    /// Angle (degrees, compass convention) the label is anchored at; equals the arrow bearing
    /// unless the label was moved aside to make room for a neighbour.
    pub anchor_deg: f64,
}

/// A hint ready for layout: the arrow bearing and the already formatted label text.
#[derive(Debug, Clone, PartialEq)]
pub struct HintLabel {
    pub bearing_deg: f64,
    pub text: String,
}

/// Everything the board needs to draw, in canvas coordinates (origin top-left, y down).
#[derive(Debug, Clone, PartialEq)]
pub struct BoardLayout {
    pub width: f64,
    pub height: f64,
    pub center: Point,
    /// Radius of the tick ring.
    pub dial_outer_radius: f64,
    /// Radius of the filled disc that holds the "?" (or the revealed city).
    pub dial_body_radius: f64,
    pub arrow_start_radius: f64,
    pub arrow_length: f64,
    pub arrows: Vec<Arrow>,
    pub labels: Vec<Label>,
    /// Corner reserved for the compass rose; nothing else may overlap it.
    pub compass: Rect,
}

impl BoardLayout {
    pub fn tip_radius(&self) -> f64 {
        self.arrow_start_radius + self.arrow_length
    }

    /// The canvas minus its safety margin: labels must stay inside.
    pub fn safe_area(&self) -> Rect {
        Rect::new(
            EDGE_MARGIN,
            EDGE_MARGIN,
            self.width - 2.0 * EDGE_MARGIN,
            self.height - 2.0 * EDGE_MARGIN,
        )
    }

    /// Everything wrong with the layout (empty when it is valid): labels outside the canvas,
    /// overlapping each other, an arrow, the dial or the compass rose.
    pub fn problems(&self) -> Vec<String> {
        let obstacles = self.obstacles();
        let mut problems = Vec::new();
        for (i, label) in self.labels.iter().enumerate() {
            if label.rect.overshoot(&obstacles.bounds) > 1e-6 {
                problems.push(format!("label {:?} leaves the canvas", label.text));
            }
            if obstacles.dial_violation(&label.rect) > 1e-6 {
                problems.push(format!("label {:?} touches the dial", label.text));
            }
            if obstacles.arrow_violation(&label.rect) > 1e-6 {
                problems.push(format!("label {:?} touches an arrow", label.text));
            }
            if label.rect.overlap_area(&self.compass, LABEL_SPACING) > 0.0 {
                problems.push(format!("label {:?} covers the compass rose", label.text));
            }
            for other in &self.labels[i + 1..] {
                if label.rect.overlap_area(&other.rect, LABEL_SPACING) > 0.0 {
                    problems.push(format!(
                        "labels {:?} and {:?} overlap",
                        label.text, other.text
                    ));
                }
            }
        }
        for arrow in &self.arrows {
            for p in arrow.head.iter().chain([&arrow.start]) {
                let inside = obstacles.bounds.distance_to_point(*p) == 0.0;
                if !inside {
                    problems.push(format!("arrow {:.0}° leaves the canvas", arrow.bearing_deg));
                }
            }
        }
        problems
    }

    fn obstacles(&self) -> Obstacles {
        Obstacles {
            center: self.center,
            dial_radius: self.dial_outer_radius + 4.0,
            arrows: self.arrows.iter().flat_map(Arrow::outline).collect(),
            bounds: self.safe_area(),
            compass: self.compass,
        }
    }
}

struct Obstacles {
    center: Point,
    dial_radius: f64,
    arrows: Vec<(Point, Point)>,
    bounds: Rect,
    compass: Rect,
}

impl Obstacles {
    fn dial_violation(&self, rect: &Rect) -> f64 {
        (self.dial_radius - rect.distance_to_point(self.center)).max(0.0)
    }

    fn arrow_violation(&self, rect: &Rect) -> f64 {
        let mut worst = 0.0_f64;
        for &(from, to) in &self.arrows {
            let length = (to.x - from.x).hypot(to.y - from.y);
            let steps = (length / 4.0).ceil().max(1.0) as usize;
            for step in 0..=steps {
                let t = step as f64 / steps as f64;
                let p = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
                worst = worst.max(ARROW_CLEARANCE - rect.distance_to_point(p));
            }
        }
        worst.max(0.0)
    }

    /// Penalty for one label alone against the fixed parts of the board.
    fn fixed_penalty(&self, rect: &Rect) -> f64 {
        10.0 * (rect.overshoot(&self.bounds)
            + self.dial_violation(rect)
            + self.arrow_violation(rect))
            + rect.overlap_area(&self.compass, LABEL_SPACING)
    }
}

fn label_size(text: &str) -> (f64, f64) {
    // 2 % safety margin on top of the measured width, rounded up to a whole pixel.
    let width = (label_text_width(text, LABEL_FONT_PX) * 1.02).ceil() + 2.0 * LABEL_PAD_X;
    (width, LABEL_HEIGHT)
}

/// Box of a label of size `w × h` anchored at `anchor_deg` on the circle of radius `ring_radius`
/// around `center`: the corner or edge of the box nearest to the arrow sits on that circle
/// point and the box extends away from the dial.
fn label_rect(center: Point, anchor_deg: f64, ring_radius: f64, w: f64, h: f64) -> Rect {
    let u = screen_vector(anchor_deg);
    let anchor = center.offset(u, ring_radius);
    // Slide the box along the tangent so the corner nearest to the arrow sits on the anchor:
    // a label on the right starts at the anchor, one on top rests on it, one at 45° has its
    // corner there.
    let ax = (u.x * 2.2).clamp(-1.0, 1.0);
    let ay = (u.y * 2.2).clamp(-1.0, 1.0);
    let c = Point::new(anchor.x + ax * w / 2.0, anchor.y + ay * h / 2.0);
    Rect::from_center(c, w, h)
}

/// Lay the board out for a canvas of `width × height` logical pixels.
///
/// `hints` may come in any order (the game gives them sorted by bearing). The result never
/// depends on the distances: arrows are all the same length.
pub fn layout_board(width: f64, height: f64, hints: &[HintLabel]) -> BoardLayout {
    let center = Point::new(width / 2.0, height / 2.0);

    // The vertical room is the scarce one: it fixes the arrow length. Horizontally the canvas is
    // always much wider than a one-line label needs.
    let dial_outer_radius = (0.17 * height).clamp(48.0, 84.0);
    let arrow_start_radius = dial_outer_radius + 8.0;
    let tip_budget = height / 2.0 - EDGE_MARGIN - LABEL_GAP - LABEL_HEIGHT;
    let arrow_length = (tip_budget - arrow_start_radius).clamp(40.0, 150.0);
    let tip_radius = arrow_start_radius + arrow_length;
    let ring_radius = tip_radius + LABEL_GAP;

    let arrows: Vec<Arrow> = hints
        .iter()
        .map(|h| Arrow::new(center, h.bearing_deg, arrow_start_radius, tip_radius))
        .collect();

    let compass = Rect::new(
        width - EDGE_MARGIN - COMPASS_SIZE,
        height - EDGE_MARGIN - COMPASS_SIZE,
        COMPASS_SIZE,
        COMPASS_SIZE,
    );

    let mut layout = BoardLayout {
        width,
        height,
        center,
        dial_outer_radius,
        dial_body_radius: dial_outer_radius * DIAL_BODY_RATIO,
        arrow_start_radius,
        arrow_length,
        arrows,
        labels: Vec::new(),
        compass,
    };

    let sizes: Vec<(f64, f64)> = hints.iter().map(|h| label_size(&h.text)).collect();
    let bearings: Vec<f64> = hints.iter().map(|h| h.bearing_deg).collect();
    let anchors = spread_anchors(&layout, ring_radius, &sizes, &bearings);

    layout.labels = hints
        .iter()
        .zip(&sizes)
        .zip(&anchors)
        .zip(&layout.arrows)
        .map(|(((hint, &(w, h)), &anchor_deg), arrow)| Label {
            text: hint.text.clone(),
            rect: label_rect(center, anchor_deg, ring_radius, w, h),
            tip: arrow.tip,
            anchor_deg,
        })
        .collect();
    layout
}

/// Angular step used when sliding a label's anchor.
const ANCHOR_STEP_DEG: f64 = 0.5;

/// Find an anchor angle per label so that no label collides with anything.
///
/// 1. *Sweeps*: labels are visited in bearing order (starting after the widest empty sector, so
///    the cluster never wraps around); each one keeps its bearing unless it collides, in which
///    case its anchor slides on in the sweep direction until it is clear. A clockwise sweep and
///    an anticlockwise sweep give two valid layouts, one drifting each way.
/// 2. Their average is a valid layout that spreads a cluster symmetrically around its arrows;
///    it is used when still valid, otherwise the sweep with the least drift.
/// 3. A short coordinate descent polishes whatever is left.
fn spread_anchors(
    layout: &BoardLayout,
    ring_radius: f64,
    sizes: &[(f64, f64)],
    bearings: &[f64],
) -> Vec<f64> {
    let n = bearings.len();
    if n == 0 {
        return Vec::new();
    }
    let obstacles = layout.obstacles();
    let center = layout.center;
    let rect_at =
        |i: usize, anchor: f64| label_rect(center, anchor, ring_radius, sizes[i].0, sizes[i].1);
    // Penalty of label `i` at `anchor` against the fixed parts of the board and the labels in
    // `others` (index, anchor).
    let penalty_of = |i: usize, anchor: f64, others: &[(usize, f64)]| {
        let rect = rect_at(i, anchor);
        let mut penalty = obstacles.fixed_penalty(&rect);
        for &(j, other_anchor) in others {
            if j != i {
                penalty += rect.overlap_area(&rect_at(j, other_anchor), LABEL_SPACING);
            }
        }
        penalty
    };

    // Visit order: ascending bearing, starting right after the widest gap.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| bearings[a].total_cmp(&bearings[b]));
    let widest_gap_end = (0..n)
        .max_by(|&a, &b| {
            let gap = |k: usize| {
                let prev = bearings[order[(k + n - 1) % n]];
                (bearings[order[k]] - prev).rem_euclid(360.0)
            };
            gap(a).total_cmp(&gap(b))
        })
        .unwrap_or(0);
    order.rotate_left(widest_gap_end);
    // Unwrapped ideal angles along the visit order (increasing).
    let mut ideal = vec![0.0; n];
    let mut previous = f64::NEG_INFINITY;
    for &i in &order {
        let mut angle = bearings[i];
        while angle < previous {
            angle += 360.0;
        }
        ideal[i] = angle;
        previous = angle;
    }

    let sweep = |direction: f64| -> Vec<f64> {
        let sequence: Vec<usize> = if direction > 0.0 {
            order.clone()
        } else {
            order.iter().rev().copied().collect()
        };
        let mut placed: Vec<(usize, f64)> = Vec::with_capacity(n);
        for &i in &sequence {
            let mut anchor = ideal[i];
            let mut moved = 0.0;
            while penalty_of(i, anchor, &placed) > 0.0 && moved < MAX_ANCHOR_SHIFT_DEG {
                anchor += direction * ANCHOR_STEP_DEG;
                moved += ANCHOR_STEP_DEG;
            }
            if moved >= MAX_ANCHOR_SHIFT_DEG {
                anchor = ideal[i];
            }
            placed.push((i, anchor));
        }
        let mut anchors = vec![0.0; n];
        for (i, anchor) in placed {
            anchors[i] = anchor;
        }
        anchors
    };
    let total_penalty = |anchors: &[f64]| -> f64 {
        let all: Vec<(usize, f64)> = anchors.iter().copied().enumerate().collect();
        (0..n).map(|i| penalty_of(i, anchors[i], &all)).sum()
    };
    let drift =
        |anchors: &[f64]| -> f64 { anchors.iter().zip(&ideal).map(|(a, b)| (a - b).abs()).sum() };

    let clockwise = sweep(1.0);
    let anticlockwise = sweep(-1.0);
    let balanced: Vec<f64> = clockwise
        .iter()
        .zip(&anticlockwise)
        .map(|(a, b)| (a + b) / 2.0)
        .collect();
    let mut anchors = [balanced, clockwise, anticlockwise]
        .into_iter()
        .min_by(|a, b| {
            total_penalty(a)
                .total_cmp(&total_penalty(b))
                .then(drift(a).total_cmp(&drift(b)))
        })
        .unwrap_or_default();

    // Polish: coordinate descent with a huge weight on collisions and a small pull towards the
    // arrow's own bearing.
    let cost = |anchors: &[f64], i: usize, anchor: f64| {
        let all: Vec<(usize, f64)> = anchors.iter().copied().enumerate().collect();
        let pull = anchor - ideal[i];
        1000.0 * penalty_of(i, anchor, &all) + 0.05 * pull * pull
    };
    for step in [ANCHOR_STEP_DEG, 1.0, 3.0] {
        for _ in 0..400 {
            let mut moved = false;
            for i in 0..n {
                let current = cost(&anchors, i, anchors[i]);
                let mut best = (current, anchors[i]);
                for candidate in [anchors[i] - step, anchors[i] + step] {
                    if (candidate - ideal[i]).abs() > MAX_ANCHOR_SHIFT_DEG {
                        continue;
                    }
                    let c = cost(&anchors, i, candidate);
                    if c < best.0 - 1e-9 {
                        best = (c, candidate);
                    }
                }
                if best.1 != anchors[i] {
                    anchors[i] = best.1;
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
    }
    anchors
}

/// Font size (logical px) for the revealed city name inside the dial's disc of radius
/// `body_radius`: as large as comfortably fits on at most two lines (a line break is allowed
/// after a space or a hyphen), never below 10 px.
pub fn dial_text_size(name: &str, body_radius: f64) -> f64 {
    const MIN_SIZE: f64 = 10.0;
    let available = body_radius * 2.0 * 0.90;
    let mut size = (body_radius * 0.5).max(MIN_SIZE);
    while size > MIN_SIZE {
        if wrapped_line_count(name, size, available).is_some_and(|lines| lines <= 2) {
            return size;
        }
        size -= 0.5;
    }
    MIN_SIZE
}

/// Greedy line count of `text` in the display font; `None` when a single unbreakable fragment
/// is wider than `available`.
fn wrapped_line_count(text: &str, font_px: f64, available: f64) -> Option<usize> {
    let mut fragments: Vec<&str> = Vec::new();
    let mut start = 0;
    for (i, ch) in text.char_indices() {
        if ch == ' ' || ch == '-' {
            fragments.push(&text[start..=i]);
            start = i + ch.len_utf8();
        }
    }
    if start < text.len() {
        fragments.push(&text[start..]);
    }

    let mut lines = 1;
    let mut line_width = 0.0;
    for fragment in fragments {
        let width = display_text_width(fragment.trim_end(), font_px);
        if width > available {
            return None;
        }
        let full = display_text_width(fragment, font_px);
        if line_width > 0.0 && line_width + width > available {
            lines += 1;
            line_width = 0.0;
        }
        line_width += full;
    }
    Some(lines)
}

// ---------------------------------------------------------------------------------------------
// Static drawings (SVG path strings)
// ---------------------------------------------------------------------------------------------

/// Tick ring of the dial, drawn in a [`DIAL_UNIT`]² box centred on (100, 100); Slint scales it
/// to the dial's size. Ticks point inwards from radius 100: long at the intercardinal points,
/// short every 10°. The north tick is separate ([`north_tick_commands`]) so it can be coloured.
pub fn dial_ticks_commands() -> String {
    let mut path = String::new();
    for step in 0..36 {
        let bearing = f64::from(step) * 10.0;
        let length = match bearing as u32 {
            0 => continue,
            90 | 180 | 270 => 15.0,
            _ => 6.0,
        };
        push_tick(&mut path, bearing, length);
    }
    for bearing in [45.0, 135.0, 225.0, 315.0] {
        push_tick(&mut path, bearing, 11.0);
    }
    path
}

/// The north tick of the dial, same box as [`dial_ticks_commands`].
pub fn north_tick_commands() -> String {
    let mut path = String::new();
    push_tick(&mut path, 0.0, 19.0);
    path
}

fn push_tick(path: &mut String, bearing: f64, length: f64) {
    let dir = screen_vector(bearing);
    let half = DIAL_UNIT / 2.0;
    let outer = Point::new(half + dir.x * half, half + dir.y * half);
    let inner = Point::new(
        half + dir.x * (half - length),
        half + dir.y * (half - length),
    );
    if !path.is_empty() {
        path.push(' ');
    }
    path.push_str(&format!(
        "M {:.2} {:.2} L {:.2} {:.2}",
        outer.x, outer.y, inner.x, inner.y
    ));
}

/// A small arrow pointing along `bearing_deg` (north = up) inside a `size × size` box, for the
/// guess history. Draw it with a fill and a stroke of the same colour.
pub fn direction_arrow_commands(bearing_deg: f64, size: f64) -> String {
    let dir = screen_vector(bearing_deg);
    let normal = Point::new(-dir.y, dir.x);
    let c = Point::new(size / 2.0, size / 2.0);
    let tail = c.offset(dir, -0.40 * size);
    let tip = c.offset(dir, 0.46 * size);
    let base = c.offset(dir, 0.10 * size);
    let left = base.offset(normal, 0.25 * size);
    let right = base.offset(normal, -0.25 * size);
    format!(
        "M {:.2} {:.2} L {:.2} {:.2} M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} Z",
        tail.x, tail.y, base.x, base.y, tip.x, tip.y, left.x, left.y, right.x, right.y
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cities::CITIES;
    use crate::geo::bearing_deg;
    use crate::search::find_exact;

    fn close(a: Point, x: f64, y: f64) -> bool {
        (a.x - x).abs() < 1e-9 && (a.y - y).abs() < 1e-9
    }

    #[test]
    fn screen_vector_follows_the_compass() {
        // north = up, east = right, south = down, west = left (screen y grows downwards)
        assert!(close(screen_vector(0.0), 0.0, -1.0));
        let east = screen_vector(90.0);
        assert!((east.x - 1.0).abs() < 1e-9 && east.y.abs() < 1e-9);
        let south = screen_vector(180.0);
        assert!(south.x.abs() < 1e-9 && (south.y - 1.0).abs() < 1e-9);
        let west = screen_vector(270.0);
        assert!((west.x + 1.0).abs() < 1e-9 && west.y.abs() < 1e-9);
        let north_east = screen_vector(45.0);
        assert!(north_east.x > 0.0 && north_east.y < 0.0);
    }

    #[test]
    fn arrows_from_paris_point_where_the_cities_are() {
        let paris = find_exact("Paris").unwrap();
        let dir = |name: &str| screen_vector(bearing_deg(paris, find_exact(name).unwrap()));
        let lille = dir("Lille");
        assert!(lille.y < -0.9 && lille.x.abs() < 0.3, "Lille is up");
        let strasbourg = dir("Strasbourg");
        assert!(
            strasbourg.x > 0.9 && strasbourg.y.abs() < 0.4,
            "Strasbourg is right"
        );
        let bordeaux = dir("Bordeaux");
        assert!(
            bordeaux.x < -0.3 && bordeaux.y > 0.3,
            "Bordeaux is down-left"
        );
    }

    #[test]
    fn km_are_rounded_and_grouped_french_style() {
        assert_eq!(format_km(0.0), "0");
        assert_eq!(format_km(391.6), "392");
        assert_eq!(format_km(391.4), "391");
        assert_eq!(format_km(999.5), "1 000");
        assert_eq!(format_km(1047.2), "1 047");
        assert_eq!(format_distance(312.4), "312 km");
    }

    #[test]
    fn hint_label_shows_distance_only_in_facile() {
        let lyon = find_exact("Lyon").unwrap();
        let hint = Hint {
            city: lyon,
            bearing_deg: 135.0,
            distance_km: 391.62,
        };
        assert_eq!(
            format_hint_label(&hint, Difficulty::Facile),
            "Lyon · 392 km"
        );
        assert_eq!(format_hint_label(&hint, Difficulty::Difficile), "Lyon");
        let far = Hint {
            distance_km: 1000.4,
            ..hint
        };
        assert_eq!(
            format_hint_label(&far, Difficulty::Facile),
            "Lyon · 1 000 km"
        );
    }

    #[test]
    fn facile_labels_carry_the_rounded_real_distance_of_every_hint() {
        use crate::game::Round;
        let mut labels = 0;
        for secret in CITIES {
            for seed in 0..5 {
                let round = Round::with_secret(secret, Difficulty::Facile, seed);
                for hint in round.hints() {
                    let label = format_hint_label(hint, Difficulty::Facile);
                    assert_eq!(
                        label,
                        format!("{} · {} km", hint.city.name, format_km(hint.distance_km))
                    );
                    // Independent of the grouping code: once the group separators are removed,
                    // the number is the core distance rounded to the nearest km.
                    let km = label
                        .rsplit(" · ")
                        .next()
                        .and_then(|tail| tail.strip_suffix(" km"))
                        .expect("label ends with ` km`")
                        .replace(' ', "");
                    assert_eq!(
                        km.parse::<u64>().expect("a whole number"),
                        hint.distance_km.round() as u64,
                        "{label}"
                    );
                    labels += 1;
                }
            }
        }
        assert_eq!(labels, CITIES.len() * 5 * Round::HINT_COUNT);
    }

    fn hint(bearing: f64, text: &str) -> HintLabel {
        HintLabel {
            bearing_deg: bearing,
            text: text.to_owned(),
        }
    }

    #[test]
    fn arrow_geometry_points_along_its_bearing_with_a_fixed_length() {
        let hints = [
            hint(0.0, "Nord"),
            hint(90.0, "Est"),
            hint(180.0, "Sud"),
            hint(270.0, "Ouest"),
        ];
        let layout = layout_board(900.0, 480.0, &hints);
        let c = layout.center;
        let tip = |i: usize| layout.arrows[i].tip;
        // up, right, down, left
        assert!((tip(0).x - c.x).abs() < 1e-9 && tip(0).y < c.y);
        assert!((tip(1).y - c.y).abs() < 1e-9 && tip(1).x > c.x);
        assert!((tip(2).x - c.x).abs() < 1e-9 && tip(2).y > c.y);
        assert!((tip(3).y - c.y).abs() < 1e-9 && tip(3).x < c.x);
        // every arrow has the same length and starts outside the dial
        for arrow in &layout.arrows {
            let length = (arrow.tip.x - arrow.start.x).hypot(arrow.tip.y - arrow.start.y);
            assert!((length - layout.arrow_length).abs() < 1e-9);
            let from_center = (arrow.start.x - c.x).hypot(arrow.start.y - c.y);
            assert!(from_center > layout.dial_outer_radius);
        }
    }

    #[test]
    fn arrow_length_does_not_depend_on_the_label_text() {
        let short = layout_board(900.0, 480.0, &[hint(30.0, "Lyon")]);
        let long = layout_board(900.0, 480.0, &[hint(30.0, "Boulogne-sur-Mer · 1 000 km")]);
        assert_eq!(short.arrow_length, long.arrow_length);
        assert_eq!(short.arrows[0], long.arrows[0]);
    }

    #[test]
    fn labels_keep_the_exact_arrow_bearing() {
        let layout = layout_board(
            800.0,
            356.0,
            &[
                hint(350.0, "Calais · 300 km"),
                hint(5.0, "Dunkerque · 310 km"),
            ],
        );
        for arrow in &layout.arrows {
            let dx = arrow.tip.x - layout.center.x;
            let dy = arrow.tip.y - layout.center.y;
            let bearing = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
            assert!((bearing - arrow.bearing_deg).abs() < 1e-6);
        }
    }

    #[test]
    fn close_bearings_still_give_a_valid_layout() {
        let layout = layout_board(
            800.0,
            356.0,
            &[
                hint(350.0, "Boulogne-sur-Mer · 1 000 km"),
                hint(5.0, "Châlons-en-Champagne · 1 000 km"),
            ],
        );
        assert!(layout.problems().is_empty(), "{:?}", layout.problems());
    }

    #[test]
    fn dial_paths_are_well_formed() {
        let ticks = dial_ticks_commands();
        // 35 ticks every 10° (north is drawn separately) + 4 intercardinal ticks
        assert_eq!(ticks.matches('M').count(), 39);
        assert_eq!(ticks.matches('L').count(), 39);
        assert!(north_tick_commands().starts_with("M 100.00 0.00"));
    }

    #[test]
    fn revealed_names_fit_the_dial() {
        use crate::cities::CITIES;
        for body in [44.0, 56.0, 66.0] {
            let short = dial_text_size("Lyon", body);
            for city in CITIES {
                let size = dial_text_size(city.name, body);
                assert!(
                    size >= 10.0 && size <= short,
                    "{} at {body}: {size}",
                    city.name
                );
                let lines = wrapped_line_count(city.name, size, body * 2.0 * 0.90);
                assert!(
                    matches!(lines, Some(1 | 2)),
                    "{} needs {lines:?} lines at {size}px in a disc of radius {body}",
                    city.name
                );
            }
        }
    }

    fn numbers(commands: &str) -> Vec<f64> {
        commands
            .split(|c: char| c.is_alphabetic() || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().unwrap())
            .collect()
    }

    #[test]
    fn direction_arrow_tip_points_along_the_bearing() {
        // numbers: tail, base, tip, left, right (x, y each); the box is 20 × 20, centre (10, 10)
        let tip = |bearing| {
            let n = numbers(&direction_arrow_commands(bearing, 20.0));
            (n[4], n[5])
        };
        let (x, y) = tip(0.0);
        assert!((x - 10.0).abs() < 1e-6 && y < 2.0, "north is up");
        let (x, y) = tip(90.0);
        assert!(x > 18.0 && (y - 10.0).abs() < 1e-6, "east is right");
        let (x, y) = tip(180.0);
        assert!((x - 10.0).abs() < 1e-6 && y > 18.0, "south is down");
        let (x, y) = tip(270.0);
        assert!(x < 2.0 && (y - 10.0).abs() < 1e-6, "west is left");
        assert!(direction_arrow_commands(45.0, 20.0).ends_with('Z'));
    }
}
