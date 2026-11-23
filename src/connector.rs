//! A connector is an ordinary Goodtype shape carrying this addon's data. This module turns a
//! route into canonical shape geometry and keeps the stored data in step with it.
use crate::{
    geometry::{EPSILON, Point, Rect, Side},
    route::{End, Route, Routing, route},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ADDON_ID: &str = "connectors";
/// Mirrors goodtype-core `MAX_SPLINE_NODES`.
pub const MAX_SPLINE_NODES: usize = 256;
/// The default outline when the host sends no pen.
pub const DEFAULT_STROKE: &str = "#16212b";
pub const DEFAULT_WIDTH: f64 = 1.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arrow {
    None,
    End,
    Both,
}

impl Arrow {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "end" => Some(Self::End),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attachment {
    pub object_id: String,
    pub port: usize,
}

/// `addonData.connectors.data`. Port sides and part bounds are cached because a reaction only
/// carries the parts that changed; the other end keeps routing around what it last saw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Data {
    pub routing: Routing,
    pub arrow: Arrow,
    pub start: Option<Attachment>,
    pub end: Option<Attachment>,
    pub start_point: Point,
    pub end_point: Point,
    #[serde(default)]
    pub bends: Vec<f64>,
    #[serde(default)]
    pub start_side: Option<Side>,
    #[serde(default)]
    pub end_side: Option<Side>,
    #[serde(default)]
    pub start_bounds: Option<Rect>,
    #[serde(default)]
    pub end_bounds: Option<Rect>,
    /// The object position this data was last written for; a moved connector moves its free ends.
    pub origin: Point,
}

impl Data {
    pub fn ends(&self) -> (End, End) {
        let end = |point, side, bounds| End {
            point,
            side,
            bounds,
        };
        (
            end(self.start_point, self.start_side, self.start_bounds),
            end(self.end_point, self.end_side, self.end_bounds),
        )
    }

    /// Unique IDs of the attached parts, in start, end order.
    pub fn refs(&self) -> Vec<String> {
        let mut refs: Vec<String> = Vec::new();
        for attachment in [&self.start, &self.end].into_iter().flatten() {
            if !refs.contains(&attachment.object_id) {
                refs.push(attachment.object_id.clone());
            }
        }
        refs
    }

    pub fn entry(&self) -> Value {
        json!({ADDON_ID: {"refs": self.refs(), "data": self}})
    }
}

/// The page area a shape's drawn outline must stay inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub point: Point,
    pub handle_in: Option<Point>,
    pub handle_out: Option<Point>,
}

impl Node {
    fn plain(point: Point) -> Self {
        Self {
            point,
            handle_in: None,
            handle_out: None,
        }
    }
}

/// Canonical shape geometry in page coordinates, before it is made local to its object.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Line { start: Point, end: Point },
    Spline { nodes: Vec<Node> },
}

/// An open arrowhead drawn into the same path: tip, one wing, back to the tip, the other wing.
/// One object keeps the connector a single editable, re-routable shape.
fn arrow_wings(tip: Point, direction: Point, stroke: f64) -> (Point, Point) {
    let length = 6.0 + 2.5 * stroke;
    let half = length * 0.42;
    let back = tip.minus(direction.scale(length));
    let normal = Point::new(-direction.y, direction.x).scale(half);
    (back.plus(normal), back.minus(normal))
}

fn shape(route: &Route, routing: Routing, arrow: Arrow, stroke: f64, page: Page) -> Option<Shape> {
    let mut nodes: Vec<Node> = match route {
        Route::Polyline(points) => points.iter().copied().map(Node::plain).collect(),
        Route::Curve {
            start,
            end,
            out,
            into,
        } => vec![
            Node {
                point: *start,
                handle_in: None,
                handle_out: Some(*out),
            },
            Node {
                point: *end,
                handle_in: Some(*into),
                handle_out: None,
            },
        ],
    };
    if nodes.len() < 2 {
        return None;
    }
    if arrow != Arrow::None {
        let tip = nodes.last()?.point;
        let (a, b) = arrow_wings(tip, route.arrival()?, stroke);
        nodes.extend([Node::plain(a), Node::plain(tip), Node::plain(b)]);
    }
    if arrow == Arrow::Both {
        let tip = nodes[0].point;
        let (a, b) = arrow_wings(tip, route.departure_reversed()?, stroke);
        nodes.splice(0..0, [Node::plain(a), Node::plain(tip), Node::plain(b)]);
    }
    fit(&mut nodes, stroke, page);
    nodes.dedup_by(|b, a| a.point.near(b.point) && a.handle_out.is_none() && b.handle_in.is_none());
    if nodes.len() < 2 || nodes.len() > MAX_SPLINE_NODES {
        return None;
    }
    Some(if nodes.len() == 2 && routing == Routing::Straight {
        Shape::Line {
            start: nodes[0].point,
            end: nodes[1].point,
        }
    } else {
        Shape::Spline { nodes }
    })
}

/// Keep the drawn outline half a stroke inside the page, as the host requires: clamp the points
/// (which keeps horizontal and vertical runs axis-aligned) and shorten handles that overshoot.
fn fit(nodes: &mut [Node], stroke: f64, page: Page) {
    let pad = stroke / 2.0;
    let clamp = |p: Point| {
        Point::new(
            p.x.clamp(pad, (page.width - pad).max(pad)),
            p.y.clamp(pad, (page.height - pad).max(pad)),
        )
    };
    for node in nodes.iter_mut() {
        node.point = clamp(node.point);
    }
    for _ in 0..12 {
        let [left, top, right, bottom] = extent(nodes);
        if left >= pad - EPSILON
            && top >= pad - EPSILON
            && right <= page.width - pad + EPSILON
            && bottom <= page.height - pad + EPSILON
        {
            return;
        }
        for node in nodes.iter_mut() {
            node.handle_in = node.handle_in.map(|h| h.scale(0.5));
            node.handle_out = node.handle_out.map(|h| h.scale(0.5));
        }
    }
    for node in nodes.iter_mut() {
        node.handle_in = None;
        node.handle_out = None;
    }
}

/// `[left, top, right, bottom]` of the drawn outline, measured by the curve extrema.
pub fn extent(nodes: &[Node]) -> [f64; 4] {
    let mut extent = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for node in nodes {
        extent = [
            extent[0].min(node.point.x),
            extent[1].min(node.point.y),
            extent[2].max(node.point.x),
            extent[3].max(node.point.y),
        ];
    }
    for pair in nodes.windows(2) {
        let (p0, p3) = (pair[0].point, pair[1].point);
        let p1 = pair[0].handle_out.map_or(p0, |h| p0.plus(h));
        let p2 = pair[1].handle_in.map_or(p3, |h| p3.plus(h));
        let (left, right) = cubic_range([p0.x, p1.x, p2.x, p3.x]);
        let (top, bottom) = cubic_range([p0.y, p1.y, p2.y, p3.y]);
        extent = [
            extent[0].min(left),
            extent[1].min(top),
            extent[2].max(right),
            extent[3].max(bottom),
        ];
    }
    extent
}

fn cubic_range([p, q, r, s]: [f64; 4]) -> (f64, f64) {
    let (mut low, mut high) = (p.min(s), p.max(s));
    let (a, b, c) = (-p + 3.0 * q - 3.0 * r + s, 2.0 * (p - 2.0 * q + r), q - p);
    let roots: Vec<f64> = if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            vec![]
        } else {
            vec![-c / b]
        }
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            vec![]
        } else {
            let root = discriminant.sqrt();
            vec![(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)]
        }
    };
    for t in roots.into_iter().filter(|t| *t > 0.0 && *t < 1.0) {
        let u = 1.0 - t;
        let value = u * u * u * p + 3.0 * u * u * t * q + 3.0 * u * t * t * r + t * t * t * s;
        low = low.min(value);
        high = high.max(value);
    }
    (low, high)
}

impl Shape {
    fn nodes(&self) -> Vec<Node> {
        match self {
            Self::Line { start, end } => vec![Node::plain(*start), Node::plain(*end)],
            Self::Spline { nodes } => nodes.clone(),
        }
    }

    /// The object origin: the top-left of the drawn outline, so local geometry starts at zero.
    pub fn origin(&self) -> Point {
        let [left, top, ..] = extent(&self.nodes());
        Point::new(left, top)
    }

    /// Canonical `ShapeGeometry` JSON relative to `origin`.
    pub fn geometry(&self, origin: Point) -> Value {
        let local = |p: Point| json!({"x": p.x - origin.x, "y": p.y - origin.y});
        let handle = |h: Option<Point>| h.map_or(Value::Null, |h| json!({"x": h.x, "y": h.y}));
        match self {
            Self::Line { start, end } => {
                json!({"kind": "line", "start": local(*start), "end": local(*end)})
            }
            Self::Spline { nodes } => json!({
                "kind": "spline",
                "closed": false,
                "nodes": nodes.iter().map(|n| json!({
                    "point": local(n.point),
                    "handleIn": handle(n.handle_in),
                    "handleOut": handle(n.handle_out),
                })).collect::<Vec<_>>(),
            }),
        }
    }
}

/// The pen the host fills a new connector's outline from. Fields mirror `ShapeStyle`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pen {
    pub stroke_color: String,
    pub stroke_width_pt: f64,
    #[serde(default = "one")]
    pub opacity: f64,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub pattern_spacing: Option<f64>,
}

fn one() -> f64 {
    1.0
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            stroke_color: DEFAULT_STROKE.into(),
            stroke_width_pt: DEFAULT_WIDTH,
            opacity: 1.0,
            pattern: None,
            pattern_spacing: None,
        }
    }
}

impl Pen {
    pub fn valid(&self) -> bool {
        let hex = self.stroke_color.len() == 7
            && self.stroke_color.starts_with('#')
            && self.stroke_color[1..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit());
        hex && self.stroke_width_pt.is_finite()
            && self.stroke_width_pt > 0.0
            && self.stroke_width_pt <= 256.0
            && (0.0..=1.0).contains(&self.opacity)
            && self
                .pattern
                .as_deref()
                .is_none_or(|p| matches!(p, "solid" | "dashed" | "dotted"))
            && self
                .pattern_spacing
                .is_none_or(|s| s.is_finite() && (0.5..=3.0).contains(&s))
    }

    pub fn style(&self) -> Value {
        let mut style = json!({
            "strokeColor": self.stroke_color,
            "strokeWidthPt": self.stroke_width_pt,
            "fillColor": null,
            "opacity": self.opacity,
        });
        if let Some(pattern) = self.pattern.as_deref().filter(|p| *p != "solid") {
            style["pattern"] = json!(pattern);
        }
        if let Some(spacing) = self.pattern_spacing.filter(|s| *s != 1.0) {
            style["patternSpacing"] = json!(spacing);
        }
        style
    }
}

/// Route `data`, store what was routed and return the shape with its origin.
pub fn build(data: &mut Data, stroke: f64, page: Page) -> Option<(Shape, Point)> {
    let (start, end) = data.ends();
    if start.point.near(end.point) || !start.point.finite() || !end.point.finite() {
        return None;
    }
    let pad = stroke / 2.0;
    let fits = |point: Point| {
        (pad..=page.width - pad).contains(&point.x) && (pad..=page.height - pad).contains(&point.y)
    };
    if (data.start.is_some() && !fits(start.point)) || (data.end.is_some() && !fits(end.point)) {
        return None;
    }
    let routed = route(data.routing, start, end, &data.bends);
    let shape = shape(&routed.route, data.routing, data.arrow, stroke, page)?;
    let origin = shape.origin();
    data.bends = routed.bends;
    data.origin = origin;
    Some((shape, origin))
}

/// Mirrors goodtype-core shape validation and the host's page-fit check for addon shapes.
pub fn validate(geometry: &Value, x: f64, y: f64, stroke: f64, page: Page) -> Result<(), String> {
    let point = |v: &Value| -> Result<Point, String> {
        let p: Point = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        p.finite()
            .then_some(p)
            .ok_or_else(|| "non-finite point".into())
    };
    let nodes = match geometry["kind"].as_str() {
        Some("line") => {
            let (start, end) = (point(&geometry["start"])?, point(&geometry["end"])?);
            if start == end {
                return Err("line needs two distinct points".into());
            }
            vec![Node::plain(start), Node::plain(end)]
        }
        Some("spline") => {
            if geometry["closed"] != json!(false) {
                return Err("connectors are open splines".into());
            }
            let nodes = geometry["nodes"].as_array().ok_or("missing nodes")?;
            if !(2..=MAX_SPLINE_NODES).contains(&nodes.len()) {
                return Err(format!("{} spline nodes", nodes.len()));
            }
            let handle = |v: &Value| (!v.is_null()).then(|| point(v)).transpose();
            nodes
                .iter()
                .map(|n| {
                    Ok(Node {
                        point: point(&n["point"])?,
                        handle_in: handle(&n["handleIn"])?,
                        handle_out: handle(&n["handleOut"])?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
        }
        _ => return Err("unknown geometry kind".into()),
    };
    let pad = stroke / 2.0;
    let [left, top, right, bottom] = extent(&nodes);
    let inside = x + left >= pad - 1e-9
        && y + top >= pad - 1e-9
        && x + right <= page.width - pad + 1e-9
        && y + bottom <= page.height - pad + 1e-9;
    if !(x.is_finite() && y.is_finite() && inside) {
        return Err("shape does not fit the page".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::tests::{Lcg, random_end};

    const A4: Page = Page {
        width: 595.0,
        height: 842.0,
    };

    fn data(routing: Routing, arrow: Arrow, start: End, end: End) -> Data {
        Data {
            routing,
            arrow,
            start: None,
            end: None,
            start_point: start.point,
            end_point: end.point,
            bends: Vec::new(),
            start_side: start.side,
            end_side: end.side,
            start_bounds: start.bounds,
            end_bounds: end.bounds,
            origin: Point::default(),
        }
    }

    #[test]
    fn straight_without_arrows_is_a_line() {
        let mut d = data(
            Routing::Straight,
            Arrow::None,
            End::free(Point::new(10.0, 20.0)),
            End::free(Point::new(110.0, 70.0)),
        );
        let (shape, origin) = build(&mut d, 1.6, A4).unwrap();
        assert_eq!(origin, Point::new(10.0, 20.0));
        assert_eq!(
            shape.geometry(origin),
            json!({"kind":"line","start":{"x":0.0,"y":0.0},"end":{"x":100.0,"y":50.0}})
        );
        assert_eq!(d.origin, origin);
    }

    #[test]
    fn arrowheads_point_into_the_end_and_out_of_the_start() {
        let mut d = data(
            Routing::Straight,
            Arrow::Both,
            End::free(Point::new(100.0, 100.0)),
            End::free(Point::new(200.0, 100.0)),
        );
        let (Shape::Spline { nodes }, _) = build(&mut d, 1.6, A4).unwrap() else {
            panic!()
        };
        let points: Vec<Point> = nodes.iter().map(|n| n.point).collect();
        assert_eq!(points.len(), 8);
        assert_eq!(points[1], Point::new(100.0, 100.0));
        assert_eq!(points[3], Point::new(100.0, 100.0));
        assert_eq!(points[6], Point::new(200.0, 100.0));
        // End wings sit behind the tip, start wings ahead of the start.
        assert!(points[5].x < 200.0 && points[7].x < 200.0);
        assert!(points[0].x > 100.0 && points[2].x > 100.0);
        assert!((points[5].y - 100.0).abs() == (points[7].y - 100.0).abs());
    }

    #[test]
    fn shapes_near_the_page_edge_are_pulled_inside() {
        let bounds = Rect {
            x: 2.0,
            y: 300.0,
            width: 40.0,
            height: 20.0,
        };
        let start = End {
            point: Point::new(2.0, 310.0),
            side: Some(Side::Left),
            bounds: Some(bounds),
        };
        for routing in [Routing::Straight, Routing::Curved, Routing::Orthogonal] {
            let mut d = data(
                routing,
                Arrow::Both,
                start,
                End::free(Point::new(5.0, 500.0)),
            );
            let (shape, origin) = build(&mut d, 2.0, A4).unwrap();
            validate(&shape.geometry(origin), origin.x, origin.y, 2.0, A4).unwrap();
        }
    }

    #[test]
    fn property_every_built_shape_passes_core_validation() {
        let mut rng = Lcg(7);
        let routings = [Routing::Straight, Routing::Curved, Routing::Orthogonal];
        let arrows = [Arrow::None, Arrow::End, Arrow::Both];
        let mut built = 0;
        for case in 0..6000 {
            let (start, end) = (random_end(&mut rng), random_end(&mut rng));
            let stroke = rng.range(0.3, 8.0);
            let mut d = data(routings[case % 3], arrows[(case / 3) % 3], start, end);
            let Some((shape, origin)) = build(&mut d, stroke, A4) else {
                assert!(start.point.near(end.point));
                continue;
            };
            let geometry = shape.geometry(origin);
            validate(&geometry, origin.x, origin.y, stroke, A4)
                .unwrap_or_else(|e| panic!("{e}: {geometry}"));
            if let Shape::Spline { nodes } = &shape {
                assert!(nodes.len() <= 64 + 6);
                for pair in nodes.windows(2) {
                    assert!(!pair[0].point.near(pair[1].point), "{geometry}");
                }
            }
            built += 1;
        }
        assert!(built > 5900);
    }

    #[test]
    fn data_round_trips_and_lists_unique_refs() {
        let mut d = data(
            Routing::Orthogonal,
            Arrow::End,
            End::free(Point::new(1.0, 2.0)),
            End::free(Point::new(3.0, 4.0)),
        );
        d.start = Some(Attachment {
            object_id: "shape-r1".into(),
            port: 1,
        });
        d.end = Some(Attachment {
            object_id: "shape-r1".into(),
            port: 0,
        });
        assert_eq!(d.refs(), ["shape-r1"]);
        let entry = d.entry();
        let back: Data = serde_json::from_value(entry[ADDON_ID]["data"].clone()).unwrap();
        assert_eq!(back, d);
        assert!(serde_json::to_string(&entry).unwrap().len() < 16 * 1024);
    }
}
