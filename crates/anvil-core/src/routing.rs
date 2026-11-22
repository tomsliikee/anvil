//! Orthogonal lines between class diagram cards that go around the other
//! cards, as VS' Class Designer draws them.
//!
//! The router builds a sparse grid from the cards: every card adds the
//! lines just outside its four sides and the lines through its centre. A
//! line leaves a card at the middle of one side, walks the grid (A* over
//! node and direction, each bend costs extra) and enters the target at the
//! middle of one side. Grid points and segments inside a card (grown by
//! the margin) are blocked. A base class above its derived class is always
//! reached from the derived card's top to the base's bottom, so several
//! derived classes share the last stretch like a UML tree.
//!
//! Small enough to run on every frame of a drag: twenty cards make a grid
//! of about 60 × 60 points.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

const SIDES: [Side; 4] = [Side::Top, Side::Bottom, Side::Left, Side::Right];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Inheritance,
    Association,
}

/// A routed line: corner points from the source card's side to the target
/// card's side, and where its label goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub points: Vec<(f64, f64)>,
    /// Middle of the longest segment.
    pub label_at: (f64, f64),
}

/// Distance between a line and the cards it passes.
pub const MARGIN: f64 = 16.0;
/// What a bend costs, in pixels of length.
const BEND: f64 = 48.0;

/// Routes every edge `(from, to, kind)` (indices into `rects`).
pub fn route_all(rects: &[Rect], edges: &[(usize, usize, EdgeKind)]) -> Vec<Route> {
    let grid = Grid::new(rects);
    edges.iter().map(|&(a, b, kind)| grid.route(rects, a, b, kind)).collect()
}

fn port(r: &Rect, side: Side) -> (f64, f64) {
    match side {
        Side::Top => (r.cx(), r.y),
        Side::Bottom => (r.cx(), r.y + r.h),
        Side::Left => (r.x, r.cy()),
        Side::Right => (r.x + r.w, r.cy()),
    }
}

fn stub(r: &Rect, side: Side) -> (f64, f64) {
    let (x, y) = port(r, side);
    match side {
        Side::Top => (x, y - MARGIN),
        Side::Bottom => (x, y + MARGIN),
        Side::Left => (x - MARGIN, y),
        Side::Right => (x + MARGIN, y),
    }
}

struct Grid {
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Free grid points, row-major (`y * xs.len() + x`).
    free: Vec<bool>,
    /// The segment to the right / below each point is free.
    right: Vec<bool>,
    down: Vec<bool>,
}

fn sorted_unique(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    v
}

impl Grid {
    fn new(rects: &[Rect]) -> Grid {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for r in rects {
            xs.extend([r.x - MARGIN, r.cx(), r.x + r.w + MARGIN]);
            ys.extend([r.y - MARGIN, r.cy(), r.y + r.h + MARGIN]);
        }
        let (xs, ys) = (sorted_unique(xs), sorted_unique(ys));
        // Grown a little less than the margin, so the lines just outside a
        // card stay free.
        let grow = MARGIN - 1.0;
        let inside = |x: f64, y: f64| rects.iter().any(|r| x > r.x - grow && x < r.x + r.w + grow && y > r.y - grow && y < r.y + r.h + grow);
        let h_blocked = |y: f64, x0: f64, x1: f64| rects.iter().any(|r| y > r.y - grow && y < r.y + r.h + grow && x0 < r.x + r.w + grow && x1 > r.x - grow);
        let v_blocked = |x: f64, y0: f64, y1: f64| rects.iter().any(|r| x > r.x - grow && x < r.x + r.w + grow && y0 < r.y + r.h + grow && y1 > r.y - grow);
        let (nx, ny) = (xs.len(), ys.len());
        let mut free = vec![false; nx * ny];
        let mut right = vec![false; nx * ny];
        let mut down = vec![false; nx * ny];
        for (j, &y) in ys.iter().enumerate() {
            for (i, &x) in xs.iter().enumerate() {
                let k = j * nx + i;
                free[k] = !inside(x, y);
                if i + 1 < nx {
                    right[k] = !h_blocked(y, x, xs[i + 1]);
                }
                if j + 1 < ny {
                    down[k] = !v_blocked(x, y, ys[j + 1]);
                }
            }
        }
        Grid { xs, ys, free, right, down }
    }

    fn index(&self, p: (f64, f64)) -> Option<usize> {
        let i = self.xs.iter().position(|&x| (x - p.0).abs() < 0.5)?;
        let j = self.ys.iter().position(|&y| (y - p.1).abs() < 0.5)?;
        let k = j * self.xs.len() + i;
        self.free[k].then_some(k)
    }

    fn route(&self, rects: &[Rect], a: usize, b: usize, kind: EdgeKind) -> Route {
        let (ra, rb) = (rects[a], rects[b]);
        let base_above = kind == EdgeKind::Inheritance && rb.y + rb.h <= ra.y;
        let (from_sides, to_sides): (&[Side], &[Side]) = if base_above { (&[Side::Top], &[Side::Bottom]) } else { (&SIDES, &SIDES) };
        let starts: Vec<(usize, Side)> = from_sides.iter().filter_map(|&s| Some((self.index(stub(&ra, s))?, s))).collect();
        let goals: Vec<(usize, Side)> = to_sides.iter().filter_map(|&s| Some((self.index(stub(&rb, s))?, s))).collect();
        let found = if a == b { None } else { self.search(&starts, &goals) };
        let points = match found {
            Some((path, s_side, g_side)) => {
                let mut pts = vec![port(&ra, s_side)];
                pts.extend(path.iter().map(|&k| (self.xs[k % self.xs.len()], self.ys[k / self.xs.len()])));
                pts.push(port(&rb, g_side));
                simplify(pts)
            }
            // Boxed in (cards overlap): a straight line between the sides
            // facing each other, as before routing existed.
            None => straight(&ra, &rb),
        };
        let label_at = longest_middle(&points);
        Route { points, label_at }
    }

    /// A* over (point, arriving direction); returns the grid path and the
    /// sides it leaves and enters.
    fn search(&self, starts: &[(usize, Side)], goals: &[(usize, Side)]) -> Option<(Vec<usize>, Side, Side)> {
        if starts.is_empty() || goals.is_empty() {
            return None;
        }
        let nx = self.xs.len();
        let n = self.free.len();
        let pos = |k: usize| (self.xs[k % nx], self.ys[k / nx]);
        let h = |k: usize| goals.iter().map(|&(g, _)| (pos(k).0 - pos(g).0).abs() + (pos(k).1 - pos(g).1).abs()).fold(f64::MAX, f64::min);
        // Direction 0..4: up, down, left, right.
        let dir_of = |s: Side| match s {
            Side::Top => 0,
            Side::Bottom => 1,
            Side::Left => 2,
            Side::Right => 3,
        };
        let mut best = vec![f64::MAX; n * 4];
        let mut prev = vec![usize::MAX; n * 4];
        let mut origin = vec![Side::Top; n * 4];
        let mut heap = BinaryHeap::new();
        let key = |c: f64| Reverse((c * 16.0) as u64);
        for &(k, s) in starts {
            let st = k * 4 + dir_of(s);
            best[st] = 0.0;
            origin[st] = s;
            heap.push((key(h(k)), st));
        }
        while let Some((_, st)) = heap.pop() {
            let (k, d) = (st / 4, st % 4);
            if let Some(&(_, gs)) = goals.iter().find(|&&(g, _)| g == k) {
                // Entering the target against its side's outward direction
                // needs that last leg to be straight: a final bend is fine.
                let mut path = vec![k];
                let mut cur = st;
                while prev[cur] != usize::MAX {
                    cur = prev[cur];
                    path.push(cur / 4);
                }
                path.reverse();
                path.dedup();
                return Some((path, origin[st], gs));
            }
            let g = best[st];
            let (i, j) = (k % nx, k / nx);
            let mut step = |to: usize, nd: usize| {
                if !self.free[to] {
                    return;
                }
                let (p, q) = (pos(k), pos(to));
                let cost = g + (p.0 - q.0).abs() + (p.1 - q.1).abs() + if nd != d { BEND } else { 0.0 };
                let ns = to * 4 + nd;
                if cost < best[ns] {
                    best[ns] = cost;
                    prev[ns] = st;
                    origin[ns] = origin[st];
                    heap.push((key(cost + h(to)), ns));
                }
            };
            if j > 0 && self.down[k - nx] {
                step(k - nx, 0);
            }
            if j + 1 < self.ys.len() && self.down[k] {
                step(k + nx, 1);
            }
            if i > 0 && self.right[k - 1] {
                step(k - 1, 2);
            }
            if i + 1 < nx && self.right[k] {
                step(k + 1, 3);
            }
        }
        None
    }
}

fn straight(a: &Rect, b: &Rect) -> Vec<(f64, f64)> {
    let (dx, dy) = (b.cx() - a.cx(), b.cy() - a.cy());
    let (sa, sb) = if dx.abs() > dy.abs() {
        if dx > 0.0 { (Side::Right, Side::Left) } else { (Side::Left, Side::Right) }
    } else if dy > 0.0 {
        (Side::Bottom, Side::Top)
    } else {
        (Side::Top, Side::Bottom)
    };
    vec![port(a, sa), port(b, sb)]
}

/// Drops points that lie on the line between their neighbours.
fn simplify(pts: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(pts.len());
    for p in pts {
        if out.last().is_some_and(|l| (l.0 - p.0).abs() < 0.01 && (l.1 - p.1).abs() < 0.01) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let collinear = ((a.0 - b.0).abs() < 0.01 && (b.0 - p.0).abs() < 0.01) || ((a.1 - b.1).abs() < 0.01 && (b.1 - p.1).abs() < 0.01);
            if collinear {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

fn longest_middle(pts: &[(f64, f64)]) -> (f64, f64) {
    pts.windows(2)
        .max_by(|a, b| seg_len(a).total_cmp(&seg_len(b)))
        .map(|w| ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0))
        .unwrap_or_else(|| pts.first().copied().unwrap_or_default())
}

fn seg_len(w: &[(f64, f64)]) -> f64 {
    (w[0].0 - w[1].0).abs() + (w[0].1 - w[1].1).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    fn orthogonal(pts: &[(f64, f64)]) -> bool {
        pts.windows(2).all(|w| (w[0].0 - w[1].0).abs() < 0.01 || (w[0].1 - w[1].1).abs() < 0.01)
    }

    /// Does the segment run through the inside of `c`?
    fn crosses(w: &[(f64, f64)], c: &Rect) -> bool {
        let (x0, x1) = (w[0].0.min(w[1].0), w[0].0.max(w[1].0));
        let (y0, y1) = (w[0].1.min(w[1].1), w[0].1.max(w[1].1));
        x0 < c.x + c.w && x1 > c.x && y0 < c.y + c.h && y1 > c.y
    }

    #[test]
    fn inheritance_goes_from_top_to_bottom() {
        let rects = [r(0.0, 300.0, 150.0, 100.0), r(0.0, 0.0, 150.0, 100.0)];
        let routes = route_all(&rects, &[(0, 1, EdgeKind::Inheritance)]);
        let p = &routes[0].points;
        assert_eq!(p.first(), Some(&(75.0, 300.0)));
        assert_eq!(p.last(), Some(&(75.0, 100.0)));
        assert_eq!(p.len(), 2, "straight up: {p:?}");
    }

    #[test]
    fn lines_go_around_a_card_in_the_way() {
        // A and B side by side with C between them.
        let rects = [r(0.0, 100.0, 120.0, 80.0), r(400.0, 100.0, 120.0, 80.0), r(200.0, 60.0, 120.0, 160.0)];
        let routes = route_all(&rects, &[(0, 1, EdgeKind::Association)]);
        let p = &routes[0].points;
        assert!(orthogonal(p), "{p:?}");
        for w in p.windows(2) {
            assert!(!crosses(w, &rects[2]), "segment {w:?} crosses the card between");
        }
        assert!(p.len() >= 4, "must bend around: {p:?}");
    }

    #[test]
    fn every_route_is_orthogonal_and_avoids_other_cards() {
        // The course diagram's cards in pixels (VS positions × 96).
        let rects = [r(336.0, 792.0, 432.0, 200.0), r(1056.0, 336.0, 144.0, 90.0), r(528.0, 480.0, 192.0, 60.0), r(744.0, 216.0, 144.0, 160.0), r(1008.0, 576.0, 144.0, 100.0)];
        let edges = [(2, 3, EdgeKind::Association), (3, 0, EdgeKind::Association), (0, 4, EdgeKind::Association), (1, 2, EdgeKind::Association)];
        for (e, route) in edges.iter().zip(route_all(&rects, &edges)) {
            assert!(orthogonal(&route.points), "{:?}", route.points);
            for w in route.points.windows(2) {
                for (k, c) in rects.iter().enumerate() {
                    if k != e.0 && k != e.1 {
                        assert!(!crosses(w, c), "edge {e:?} crosses card {k}: {:?}", route.points);
                    }
                }
            }
        }
    }

    #[test]
    fn a_card_inside_another_falls_back_to_a_straight_line() {
        let rects = [r(0.0, 0.0, 300.0, 300.0), r(40.0, 40.0, 100.0, 100.0)];
        let routes = route_all(&rects, &[(0, 1, EdgeKind::Association)]);
        assert_eq!(routes[0].points.len(), 2);
    }

    #[test]
    fn twenty_cards_route_fast_enough_for_a_drag() {
        let rects: Vec<Rect> = (0..20).map(|i| r((i % 5) as f64 * 260.0, (i / 5) as f64 * 240.0, 180.0, 140.0)).collect();
        let edges: Vec<(usize, usize, EdgeKind)> = (1..20).map(|i| (i, (i * 7) % 20, if i % 2 == 0 { EdgeKind::Inheritance } else { EdgeKind::Association })).collect();
        let t = std::time::Instant::now();
        let routes = route_all(&rects, &edges);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(routes.len(), 19);
        // Generous for debug builds on slow CI; release is far below 1 ms.
        assert!(ms < 40.0, "routing took {ms:.1} ms");
    }
}
