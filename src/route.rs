//! Pure, deterministic connector routing in page points.
use crate::geometry::{EPSILON, Point, Rect, Side};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::BinaryHeap};

/// How far an orthogonal connector stays away from the parts it attaches to.
pub const CLEARANCE: f64 = 12.0;
/// One bend costs as much as this much extra length, so routes prefer few bends.
const BEND_COST: f64 = 24.0;
/// Routes never exceed this many points.
pub const MAX_ROUTE_POINTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Routing {
    Straight,
    Curved,
    Orthogonal,
}

impl Routing {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "straight" => Some(Self::Straight),
            "curved" => Some(Self::Curved),
            "orthogonal" => Some(Self::Orthogonal),
            _ => None,
        }
    }
}

/// One end of a connector: where it is, and for an attached end, the side it leaves from and
/// the part's bounds to keep clear of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct End {
    pub point: Point,
    pub side: Option<Side>,
    pub bounds: Option<Rect>,
}

impl End {
    pub fn free(point: Point) -> Self {
        Self {
            point,
            side: None,
            bounds: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    /// Straight segments through these points.
    Polyline(Vec<Point>),
    /// One cubic Bézier; handles are relative to their endpoint, as in Goodtype splines.
    Curve {
        start: Point,
        end: Point,
        out: Point,
        into: Point,
    },
}

impl Route {
    /// Unit direction the connector travels as it arrives at its end.
    pub fn arrival(&self) -> Option<Point> {
        match self {
            Self::Polyline(points) => {
                let [.., a, b] = points.as_slice() else {
                    return None;
                };
                b.minus(*a).unit()
            }
            Self::Curve {
                start, end, into, ..
            } => into.scale(-1.0).unit().or_else(|| end.minus(*start).unit()),
        }
    }
    /// Unit direction pointing backwards out of the start, i.e. how an arrow at the start points.
    pub fn departure_reversed(&self) -> Option<Point> {
        match self {
            Self::Polyline(points) => {
                let [a, b, ..] = points.as_slice() else {
                    return None;
                };
                a.minus(*b).unit()
            }
            Self::Curve {
                start, end, out, ..
            } => out.scale(-1.0).unit().or_else(|| start.minus(*end).unit()),
        }
    }
}

/// The routed connector plus the middle-segment offsets that were honoured.
#[derive(Clone, Debug, PartialEq)]
pub struct Routed {
    pub route: Route,
    pub bends: Vec<f64>,
}

/// Route a connector. `bends` are offsets of an orthogonal route's middle segments from their
/// automatic position; they are kept only while the route keeps the same shape and stays clear.
pub fn route(routing: Routing, start: End, end: End, bends: &[f64]) -> Routed {
    match routing {
        Routing::Straight => Routed {
            route: Route::Polyline(vec![start.point, end.point]),
            bends: Vec::new(),
        },
        Routing::Curved => Routed {
            route: curved(start, end),
            bends: Vec::new(),
        },
        Routing::Orthogonal => {
            let (points, bends) = orthogonal(start, end, bends);
            Routed {
                route: Route::Polyline(points),
                bends,
            }
        }
    }
}

fn curved(start: End, end: End) -> Route {
    let span = end.point.minus(start.point);
    let reach = (span.length() * 0.45).clamp(10.0, 160.0);
    let out = start
        .side
        .map_or(span.scale(1.0 / 3.0), |side| side.normal().scale(reach));
    let into = end
        .side
        .map_or(span.scale(-1.0 / 3.0), |side| side.normal().scale(reach));
    Route::Curve {
        start: start.point,
        end: end.point,
        out,
        into,
    }
}

const DIRECTIONS: [Point; 4] = [
    Point::new(1.0, 0.0),
    Point::new(0.0, 1.0),
    Point::new(-1.0, 0.0),
    Point::new(0.0, -1.0),
];
const NO_DIRECTION: usize = 4;

fn direction_index(side: Side) -> usize {
    match side {
        Side::Right => 0,
        Side::Bottom => 1,
        Side::Left => 2,
        Side::Top => 3,
    }
}

fn opposite(direction: usize) -> usize {
    (direction + 2) % 4
}

fn orthogonal(start: End, end: End, bends: &[f64]) -> (Vec<Point>, Vec<f64>) {
    for stub in [CLEARANCE, CLEARANCE / 2.0, 0.0] {
        let obstacles: Vec<Rect> = if stub > 0.0 {
            [start.bounds, end.bounds]
                .into_iter()
                .flatten()
                .filter(Rect::valid)
                .map(|bounds| bounds.inflate(stub))
                .collect()
        } else {
            Vec::new()
        };
        if let Some(points) = search(start, end, stub, &obstacles)
            && points.len() <= MAX_ROUTE_POINTS
        {
            let (from, to) = (stub_point(start, stub), stub_point(end, stub));
            let points = center(points, &obstacles, from, to);
            return apply_bends(points, bends, &obstacles, from, to);
        }
    }
    (elbow(start, end), Vec::new())
}

/// Where the connector leaves (or reaches) an attached part: `stub` beyond the part's edge on
/// the port's side, along the side's normal. A port inside the part's outline (a lead end that
/// does not touch the bounds) still clears the part.
fn stub_point(end: End, stub: f64) -> Point {
    let Some(side) = end.side else {
        return end.point;
    };
    let p = end.point;
    match end.bounds.filter(Rect::valid) {
        None => p.plus(side.normal().scale(stub)),
        Some(b) => match side {
            Side::Left => Point::new(p.x.min(b.x) - stub, p.y),
            Side::Right => Point::new(p.x.max(b.right()) + stub, p.y),
            Side::Top => Point::new(p.x, p.y.min(b.y) - stub),
            Side::Bottom => Point::new(p.x, p.y.max(b.bottom()) + stub),
        },
    }
}

#[derive(PartialEq)]
struct Entry {
    cost: f64,
    order: usize,
    state: usize,
}

impl Eq for Entry {}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // A min-heap on cost, ties broken by insertion order for determinism.
        other
            .cost
            .total_cmp(&self.cost)
            .then(other.order.cmp(&self.order))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn axis_values(values: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let mut values: Vec<f64> = values.into_iter().filter(|v| v.is_finite()).collect();
    values.sort_by(f64::total_cmp);
    values.dedup_by(|a, b| (*a - *b).abs() <= EPSILON);
    let mut all = values.clone();
    all.extend(values.windows(2).map(|pair| (pair[0] + pair[1]) / 2.0));
    all.sort_by(f64::total_cmp);
    all
}

fn index_of(values: &[f64], value: f64) -> Option<usize> {
    values.iter().position(|v| (v - value).abs() <= EPSILON)
}

/// Shortest Manhattan path with a bend penalty over the grid that the stubs and the inflated
/// part bounds span. The first move leaves along the start side, the last one enters along the
/// end side, and a path never doubles back on itself.
fn search(start: End, end: End, stub: f64, obstacles: &[Rect]) -> Option<Vec<Point>> {
    let (from, to) = (stub_point(start, stub), stub_point(end, stub));
    let xs = axis_values(
        obstacles
            .iter()
            .flat_map(|r| [r.x, r.right()])
            .chain([from.x, to.x]),
    );
    let ys = axis_values(
        obstacles
            .iter()
            .flat_map(|r| [r.y, r.bottom()])
            .chain([from.y, to.y]),
    );
    let (columns, rows) = (xs.len(), ys.len());
    let node = |index: usize| Point::new(xs[index % columns], ys[index / columns]);
    let blocked = |point: Point| obstacles.iter().any(|r| r.contains_strictly(point));
    let source = index_of(&ys, from.y)? * columns + index_of(&xs, from.x)?;
    let target = index_of(&ys, to.y)? * columns + index_of(&xs, to.x)?;
    let first = start.side.map_or(NO_DIRECTION, direction_index);
    // Entering the end port means travelling against its side's normal.
    let last = end.side.map(|side| opposite(direction_index(side)));

    let states = columns * rows * 5;
    let goal = states;
    let mut best = vec![f64::INFINITY; states + 1];
    let mut previous = vec![usize::MAX; states + 1];
    let mut heap = BinaryHeap::new();
    let mut order = 0;
    let initial = source * 5 + first;
    best[initial] = 0.0;
    heap.push(Entry {
        cost: 0.0,
        order,
        state: initial,
    });
    while let Some(Entry { cost, state, .. }) = heap.pop() {
        if cost > best[state] {
            continue;
        }
        if state == goal {
            break;
        }
        let (index, heading) = (state / 5, state % 5);
        let mut relax = |next: usize, cost: f64, heap: &mut BinaryHeap<Entry>| {
            if cost < best[next] - 1e-9 {
                best[next] = cost;
                previous[next] = state;
                order += 1;
                heap.push(Entry {
                    cost,
                    order,
                    state: next,
                });
            }
        };
        if index == target {
            let penalty = match (last, heading) {
                (None, _) | (_, NO_DIRECTION) => Some(0.0),
                (Some(required), heading) if heading == required => Some(0.0),
                (Some(required), heading) if heading == opposite(required) => None,
                _ => Some(BEND_COST),
            };
            if let Some(penalty) = penalty {
                relax(goal, cost + penalty, &mut heap);
            }
        }
        let (column, row) = (index % columns, index / columns);
        for (direction, step) in DIRECTIONS.iter().enumerate() {
            if heading != NO_DIRECTION && direction == opposite(heading) {
                continue;
            }
            let (Some(c), Some(r)) = (
                column.checked_add_signed(step.x as isize),
                row.checked_add_signed(step.y as isize),
            ) else {
                continue;
            };
            if c >= columns || r >= rows {
                continue;
            }
            let next = r * columns + c;
            let (a, b) = (node(index), node(next));
            if (blocked(b) && next != target) || obstacles.iter().any(|rect| rect.blocks(a, b)) {
                continue;
            }
            let turn = heading != NO_DIRECTION && heading != direction;
            let step_cost = a.minus(b).length() + if turn { BEND_COST } else { 0.0 };
            relax(next * 5 + direction, cost + step_cost, &mut heap);
        }
    }
    if !best[goal].is_finite() {
        return None;
    }
    let mut cells = Vec::new();
    let mut state = previous[goal];
    while state != usize::MAX {
        cells.push(node(state / 5));
        state = previous[state];
    }
    cells.reverse();
    let mut points = vec![start.point];
    points.extend(cells);
    points.push(end.point);
    Some(simplify(points))
}

/// Drop repeated points and merge collinear runs, so every segment is a real bend-to-bend run.
pub fn simplify(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for point in points {
        if out.last().is_some_and(|last| last.near(point)) {
            continue;
        }
        if let [.., a, b] = out.as_slice() {
            let vertical = (a.x - b.x).abs() <= EPSILON && (b.x - point.x).abs() <= EPSILON;
            let horizontal = (a.y - b.y).abs() <= EPSILON && (b.y - point.y).abs() <= EPSILON;
            if vertical || horizontal {
                out.pop();
                if out.last().is_some_and(|last| last.near(point)) {
                    continue;
                }
            }
        }
        out.push(point);
    }
    out
}

fn horizontal(a: Point, b: Point) -> bool {
    (a.y - b.y).abs() <= EPSILON
}

fn sign(value: f64) -> i8 {
    if value > EPSILON {
        1
    } else if value < -EPSILON {
        -1
    } else {
        0
    }
}

/// Whether `candidate` keeps the shape of `original` (same run directions, no run vanishing),
/// still leaves through both stubs, and keeps its runs outside the inflated parts.
fn acceptable(
    original: &[Point],
    candidate: &[Point],
    obstacles: &[Rect],
    from: Point,
    to: Point,
) -> bool {
    let n = candidate.len();
    if n != original.len() || n < 2 {
        return false;
    }
    let same_shape = original
        .windows(2)
        .zip(candidate.windows(2))
        .all(|(old, new)| {
            let (before, after) = (old[1].minus(old[0]), new[1].minus(new[0]));
            sign(before.x) == sign(after.x)
                && sign(before.y) == sign(after.y)
                && after.length() > EPSILON
        });
    let dot = |a: Point, b: Point| a.x * b.x + a.y * b.y;
    let stubs_kept = dot(candidate[1].minus(from), candidate[1].minus(candidate[0])) >= -EPSILON
        && dot(
            to.minus(candidate[n - 2]),
            candidate[n - 1].minus(candidate[n - 2]),
        ) >= -EPSILON;
    if !same_shape || !stubs_kept {
        return false;
    }
    // The runs outside the stubs: from the first stub point to the last one.
    let mut runs: Vec<(Point, Point)> = candidate.windows(2).map(|w| (w[0], w[1])).collect();
    runs[0].0 = from;
    runs[n - 2].1 = to;
    if n == 2 {
        runs[0] = (from, to);
    }
    runs.iter()
        .filter(|(a, b)| !a.near(*b))
        .all(|(a, b)| !obstacles.iter().any(|r| r.blocks(*a, *b)))
}

/// Move each middle run of a Z-shaped step to halfway between its neighbours, so equal-cost
/// routes settle on the balanced one and follow a moving part smoothly.
fn center(points: Vec<Point>, obstacles: &[Rect], from: Point, to: Point) -> Vec<Point> {
    let mut points = points;
    for k in 1..points.len().saturating_sub(2) {
        let (before, after) = (
            points[k].minus(points[k - 1]),
            points[k + 2].minus(points[k + 1]),
        );
        if sign(before.x) != sign(after.x) || sign(before.y) != sign(after.y) {
            continue;
        }
        let mut candidate = points.clone();
        if horizontal(points[k], points[k + 1]) {
            let y = (points[k - 1].y + points[k + 2].y) / 2.0;
            candidate[k].y = y;
            candidate[k + 1].y = y;
        } else {
            let x = (points[k - 1].x + points[k + 2].x) / 2.0;
            candidate[k].x = x;
            candidate[k + 1].x = x;
        }
        if acceptable(&points, &candidate, obstacles, from, to) {
            points = candidate;
        }
    }
    points
}

/// Shift each middle run by its stored offset if the route keeps its shape and stays clear;
/// otherwise use the automatic route and forget the offsets.
fn apply_bends(
    points: Vec<Point>,
    bends: &[f64],
    obstacles: &[Rect],
    from: Point,
    to: Point,
) -> (Vec<Point>, Vec<f64>) {
    let middles = points.len().saturating_sub(3);
    if bends.is_empty() || bends.len() != middles || bends.iter().any(|b| !b.is_finite()) {
        return (points, Vec::new());
    }
    let mut shifted = points.clone();
    for (index, offset) in bends.iter().enumerate() {
        // Middle run `index` is segment index + 1, from point index + 1 to index + 2.
        let (a, b) = (index + 1, index + 2);
        if horizontal(points[a], points[b]) {
            shifted[a].y += offset;
            shifted[b].y += offset;
        } else {
            shifted[a].x += offset;
            shifted[b].x += offset;
        }
    }
    if acceptable(&points, &shifted, obstacles, from, to) {
        (shifted, bends.to_vec())
    } else {
        (points, Vec::new())
    }
}

/// The last-resort route: out along the stubs and across at the midpoint. Always orthogonal.
fn elbow(start: End, end: End) -> Vec<Point> {
    let (from, to) = (stub_point(start, CLEARANCE), stub_point(end, CLEARANCE));
    let vertical_first = matches!(start.side, Some(Side::Top | Side::Bottom));
    let middle = if vertical_first {
        let y = (from.y + to.y) / 2.0;
        [Point::new(from.x, y), Point::new(to.x, y)]
    } else {
        let x = (from.x + to.x) / 2.0;
        [Point::new(x, from.y), Point::new(x, to.y)]
    };
    simplify(vec![start.point, from, middle[0], middle[1], to, end.point])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn part(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn attached(point: Point, bounds: Rect) -> End {
        End {
            point,
            side: Side::of(point, bounds),
            bounds: Some(bounds),
        }
    }

    fn polyline(routed: &Routed) -> &[Point] {
        match &routed.route {
            Route::Polyline(points) => points,
            Route::Curve { .. } => panic!("expected a polyline"),
        }
    }

    /// Checks every property an orthogonal route must have.
    fn assert_orthogonal(points: &[Point], start: End, end: End) {
        assert!(
            points.len() >= 2 && points.len() <= MAX_ROUTE_POINTS,
            "{points:?}"
        );
        assert_eq!(points[0], start.point);
        assert_eq!(*points.last().unwrap(), end.point);
        for segment in points.windows(2) {
            let (a, b) = (segment[0], segment[1]);
            assert!(a.finite() && b.finite());
            assert!(
                (a.x - b.x).abs() <= EPSILON || (a.y - b.y).abs() <= EPSILON,
                "diagonal segment {a:?} {b:?} in {points:?}"
            );
            assert!(
                a.minus(b).length() > EPSILON,
                "zero-length segment in {points:?}"
            );
        }
    }

    #[test]
    fn facing_ports_route_with_one_middle_segment() {
        let left = part(100.0, 100.0, 48.0, 16.0);
        let right = part(300.0, 160.0, 48.0, 16.0);
        let start = attached(Point::new(148.0, 108.0), left);
        let end = attached(Point::new(300.0, 168.0), right);
        let routed = route(Routing::Orthogonal, start, end, &[]);
        let points = polyline(&routed);
        assert_orthogonal(points, start, end);
        assert_eq!(
            points,
            [
                Point::new(148.0, 108.0),
                Point::new(224.0, 108.0),
                Point::new(224.0, 168.0),
                Point::new(300.0, 168.0)
            ]
        );
    }

    #[test]
    fn route_leaves_and_enters_perpendicular_to_the_port_side() {
        let a = part(100.0, 100.0, 40.0, 40.0);
        let b = part(300.0, 300.0, 40.0, 40.0);
        // Start on the top of A, end on the left of B.
        let start = attached(Point::new(120.0, 100.0), a);
        let end = attached(Point::new(300.0, 320.0), b);
        let points = polyline(&route(Routing::Orthogonal, start, end, &[])).to_vec();
        assert_orthogonal(&points, start, end);
        assert!(points[1].y < points[0].y && points[1].x == points[0].x);
        let [.., before, last] = points.as_slice() else {
            unreachable!()
        };
        assert!(before.x < last.x && before.y == last.y);
        for segment in points.windows(2).skip(1).take(points.len() - 3) {
            assert!(!a.blocks(segment[0], segment[1]) && !b.blocks(segment[0], segment[1]));
        }
    }

    #[test]
    fn ports_facing_away_go_around_the_parts() {
        let a = part(100.0, 100.0, 40.0, 20.0);
        let b = part(300.0, 100.0, 40.0, 20.0);
        // Leave A to the left and enter B from the right: the route must wrap around both.
        let start = attached(Point::new(100.0, 110.0), a);
        let end = attached(Point::new(340.0, 110.0), b);
        let points = polyline(&route(Routing::Orthogonal, start, end, &[])).to_vec();
        assert_orthogonal(&points, start, end);
        for segment in points.windows(2) {
            for rect in [a, b] {
                assert!(!rect.blocks(segment[0], segment[1]), "{points:?}");
            }
        }
        assert!(points.len() <= 8, "{points:?}");
    }

    #[test]
    fn small_moves_shift_the_route_continuously() {
        let a = part(100.0, 100.0, 48.0, 16.0);
        let start = attached(Point::new(148.0, 108.0), a);
        let base = polyline(&route(
            Routing::Orthogonal,
            start,
            attached(Point::new(300.0, 168.0), part(300.0, 160.0, 48.0, 16.0)),
            &[],
        ))
        .to_vec();
        let moved = polyline(&route(
            Routing::Orthogonal,
            start,
            attached(Point::new(302.0, 169.0), part(302.0, 161.0, 48.0, 16.0)),
            &[],
        ))
        .to_vec();
        assert_eq!(base.len(), moved.len());
        for (a, b) in base.iter().zip(&moved) {
            assert!(a.minus(*b).length() <= 2.3, "{base:?} vs {moved:?}");
        }
    }

    #[test]
    fn stored_bends_are_honoured_only_while_valid() {
        let left = part(100.0, 100.0, 48.0, 16.0);
        let right = part(300.0, 160.0, 48.0, 16.0);
        let start = attached(Point::new(148.0, 108.0), left);
        let end = attached(Point::new(300.0, 168.0), right);
        let routed = route(Routing::Orthogonal, start, end, &[30.0]);
        assert_eq!(routed.bends, [30.0]);
        assert_eq!(polyline(&routed)[1], Point::new(254.0, 108.0));
        // Pushing the middle segment past the end's stub would double back: dropped.
        let routed = route(Routing::Orthogonal, start, end, &[90.0]);
        assert!(routed.bends.is_empty());
        assert_eq!(polyline(&routed)[1], Point::new(224.0, 108.0));
        // A different shape forgets them too.
        assert!(
            route(Routing::Orthogonal, start, end, &[1.0, 2.0])
                .bends
                .is_empty()
        );
    }

    #[test]
    fn free_ends_route_without_stubs() {
        let start = End::free(Point::new(10.0, 10.0));
        let end = End::free(Point::new(80.0, 50.0));
        let points = polyline(&route(Routing::Orthogonal, start, end, &[])).to_vec();
        assert_orthogonal(&points, start, end);
        assert_eq!(points.len(), 3);
        let aligned = End::free(Point::new(80.0, 10.0));
        assert_eq!(
            polyline(&route(Routing::Orthogonal, start, aligned, &[])),
            [start.point, aligned.point]
        );
    }

    #[test]
    fn curves_leave_along_the_port_normal() {
        let a = part(100.0, 100.0, 48.0, 16.0);
        let b = part(300.0, 200.0, 48.0, 16.0);
        let start = attached(Point::new(148.0, 108.0), a);
        let end = attached(Point::new(324.0, 200.0), b);
        let Route::Curve {
            start: s,
            end: e,
            out,
            into,
        } = route(Routing::Curved, start, end, &[]).route
        else {
            panic!()
        };
        assert_eq!((s, e), (start.point, end.point));
        assert!(out.x > 0.0 && out.y == 0.0);
        assert!(into.y < 0.0 && into.x == 0.0);
        let free = route(
            Routing::Curved,
            End::free(Point::new(0.0, 0.0)),
            End::free(Point::new(30.0, 0.0)),
            &[],
        );
        assert_eq!(free.route.arrival(), Some(Point::new(1.0, 0.0)));
    }

    /// A small deterministic generator, so property tests need no extra dependency.
    pub(crate) struct Lcg(pub u64);
    impl Lcg {
        pub fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        pub fn range(&mut self, low: f64, high: f64) -> f64 {
            low + (high - low) * self.next()
        }
    }

    /// A random part and a port on one of its sides, possibly a corner or an inner point.
    pub(crate) fn random_end(rng: &mut Lcg) -> End {
        let bounds = part(
            rng.range(0.0, 500.0).round(),
            rng.range(0.0, 700.0).round(),
            rng.range(0.0, 120.0).round(),
            rng.range(0.0, 120.0).round(),
        );
        let t = rng.range(0.0, 1.0);
        let point = match (rng.next() * 6.0) as u32 {
            0 => Point::new(bounds.x, bounds.y + t * bounds.height),
            1 => Point::new(bounds.right(), bounds.y + t * bounds.height),
            2 => Point::new(bounds.x + t * bounds.width, bounds.y),
            3 => Point::new(bounds.x + t * bounds.width, bounds.bottom()),
            4 => bounds.center(),
            _ => return End::free(Point::new(rng.range(0.0, 595.0), rng.range(0.0, 842.0))),
        };
        attached(point, bounds)
    }

    #[test]
    fn property_orthogonal_routes_are_clean_for_random_parts() {
        let mut rng = Lcg(0x5eed);
        for _ in 0..4000 {
            let (start, end) = (random_end(&mut rng), random_end(&mut rng));
            if start.point.near(end.point) {
                continue;
            }
            let bends: Vec<f64> = if rng.next() < 0.3 {
                vec![rng.range(-40.0, 40.0)]
            } else {
                Vec::new()
            };
            let routed = route(Routing::Orthogonal, start, end, &bends);
            assert_orthogonal(polyline(&routed), start, end);
            // Routing is a pure function.
            assert_eq!(routed, route(Routing::Orthogonal, start, end, &bends));
        }
    }

    #[test]
    fn property_routes_avoid_well_separated_parts() {
        let mut rng = Lcg(42);
        let mut checked = 0;
        for _ in 0..3000 {
            let (start, end) = (random_end(&mut rng), random_end(&mut rng));
            let (Some(a), Some(b)) = (start.bounds, end.bounds) else {
                continue;
            };
            if start.side.is_none() || end.side.is_none() {
                continue;
            }
            let (ia, ib) = (a.inflate(2.0 * CLEARANCE), b.inflate(2.0 * CLEARANCE));
            let apart =
                ia.right() < ib.x || ib.right() < ia.x || ia.bottom() < ib.y || ib.bottom() < ia.y;
            if !apart {
                continue;
            }
            let points = polyline(&route(Routing::Orthogonal, start, end, &[])).to_vec();
            for segment in points
                .windows(2)
                .skip(1)
                .take(points.len().saturating_sub(3))
            {
                for rect in [a, b] {
                    assert!(
                        !rect.blocks(segment[0], segment[1]),
                        "{points:?} {a:?} {b:?}"
                    );
                }
            }
            // The first and last runs leave and enter along the port normal.
            let first = points[1].minus(points[0]).unit().unwrap();
            assert!(
                first.near(start.side.unwrap().normal()),
                "{points:?} {start:?} {end:?}"
            );
            let last = points[points.len() - 2]
                .minus(points[points.len() - 1])
                .unit()
                .unwrap();
            assert!(last.near(end.side.unwrap().normal()), "{points:?}");
            checked += 1;
        }
        assert!(checked > 500, "only {checked} cases");
    }
}
