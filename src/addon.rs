//! The three requests that create and maintain connectors: `tool/apply`, `page/changed` and the
//! `change-routing` own-object command. Each returns a `page-batch-v0` result.
use crate::{
    connector::{
        ADDON_ID, Arrow, Attachment, DEFAULT_WIDTH, Data, Page, Pen, Shape, build, caps_fit,
        legacy_tips, read_nodes, validate,
    },
    geometry::{Point, Rect, Side},
    route::Routing,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

pub const TOOL: &str = "connect";
pub const COMMAND: &str = "change-routing";
/// page-batch-v0 accepts at most this many operations.
pub const MAX_OPERATIONS: usize = 32;

/// A request the addon refuses, mapped to a JSON-RPC error.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// `-32602`: the parameters do not match the contract.
    Invalid(&'static str),
    /// `-32001`: well-formed, but no connector can be made (for example both ends coincide).
    Failed(&'static str),
}

type Outcome = Result<Value, Refusal>;

fn invalid<E>(_: E) -> Refusal {
    Refusal::Invalid("Invalid connector request")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageParams {
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    width_pt: f64,
    height_pt: f64,
}

impl PageParams {
    fn page(&self) -> Result<Page, Refusal> {
        let ok = |v: f64| v.is_finite() && v > 0.0 && v <= 100_000.0;
        if ok(self.width_pt) && ok(self.height_pt) {
            Ok(Page {
                width: self.width_pt,
                height: self.height_pt,
            })
        } else {
            Err(Refusal::Invalid("Invalid page size"))
        }
    }
}

/// What the addon reads from an object snapshot. Snapshots may carry more (type, transform,
/// geometry); only bounds and page-space ports matter for routing.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    id: String,
    bounds: Rect,
    #[serde(default)]
    ports: Vec<Point>,
}

fn snapshots(
    values: impl IntoIterator<Item = Value>,
) -> Result<HashMap<String, Snapshot>, Refusal> {
    let mut map = HashMap::new();
    for value in values {
        let snapshot: Snapshot = serde_json::from_value(value).map_err(invalid)?;
        if !snapshot.bounds.valid()
            || snapshot.ports.len() > 32
            || snapshot.ports.iter().any(|p| !p.finite())
        {
            return Err(Refusal::Invalid("Invalid object snapshot"));
        }
        map.insert(snapshot.id.clone(), snapshot);
    }
    Ok(map)
}

/// Where an attachment currently is: its port in page coordinates and the part's bounds.
/// A port at the centre of its part has no side and leaves in whichever direction is shortest.
fn locate(
    attachment: &Attachment,
    parts: &HashMap<String, Snapshot>,
) -> Option<(Point, Option<Side>, Rect)> {
    let part = parts.get(&attachment.object_id)?;
    let port = *part.ports.get(attachment.port)?;
    Some((port, Side::of(port, part.bounds), part.bounds))
}

fn settings(map: &Map<String, Value>) -> Result<(Routing, Arrow), Refusal> {
    let mut routing = Routing::Orthogonal;
    let mut arrow = Arrow::None;
    for (key, value) in map {
        let value = value.as_str().ok_or(Refusal::Invalid("Invalid setting"))?;
        match key.as_str() {
            "routing" => {
                routing = Routing::parse(value).ok_or(Refusal::Invalid("Invalid routing"))?
            }
            "arrow" => arrow = Arrow::parse(value).ok_or(Refusal::Invalid("Invalid arrow"))?,
            _ => return Err(Refusal::Invalid("Unknown setting")),
        }
    }
    Ok((routing, arrow))
}

fn batch(operations: Vec<Value>) -> Value {
    json!({"batch": {"operations": operations}})
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolPoint {
    x: f64,
    y: f64,
    attach: Option<Attachment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolApply {
    tool: String,
    settings: Map<String, Value>,
    page: PageParams,
    points: Vec<ToolPoint>,
    objects: Vec<Value>,
    #[serde(default)]
    style: Option<Value>,
    /// Compatibility with older hosts.
    #[serde(default)]
    pen: Option<Pen>,
}

/// `tool/apply` for the `connect` tool: one new connector between the two released points.
pub fn tool_apply(params: Value) -> Outcome {
    let request: ToolApply = serde_json::from_value(params).map_err(invalid)?;
    if request.tool != TOOL || request.points.len() != 2 {
        return Err(Refusal::Invalid("Unknown tool or wrong point count"));
    }
    let page = request.page.page()?;
    let (routing, arrow) = settings(&request.settings)?;
    let supplied_pen = request.pen.is_some();
    let pen = request.pen.unwrap_or_default();
    if !pen.valid() {
        return Err(Refusal::Invalid("Invalid pen"));
    }
    let stroke_width = match &request.style {
        Some(style) => style
            .get("strokeWidthPt")
            .and_then(Value::as_f64)
            .filter(|width| (0.05..=64.0).contains(width))
            .ok_or(Refusal::Invalid("Invalid style"))?,
        None => pen.stroke_width_pt,
    };
    let parts = snapshots(request.objects)?;
    let mut ends = Vec::with_capacity(2);
    for point in &request.points {
        let fallback = Point::new(point.x, point.y);
        if !fallback.finite() {
            return Err(Refusal::Invalid("Invalid point"));
        }
        ends.push(match &point.attach {
            Some(attachment) => {
                let (port, side, bounds) =
                    locate(attachment, &parts).ok_or(Refusal::Invalid("Unknown port"))?;
                (Some(attachment.clone()), port, side, Some(bounds))
            }
            None => (None, fallback, None, None),
        });
    }
    let [
        (start, start_point, start_side, start_bounds),
        (end, end_point, end_side, end_bounds),
    ] = <[_; 2]>::try_from(ends).expect("two points");
    let mut data = Data {
        routing,
        version: 2,
        start,
        end,
        start_point,
        end_point,
        bends: Vec::new(),
        start_side,
        end_side,
        start_bounds,
        end_bounds,
        origin: Point::default(),
    };
    let (shape, origin) = build(&mut data, stroke_width, page)
        .ok_or(Refusal::Failed("The connector ends coincide"))?;
    let geometry = shape.geometry(origin);
    validate(&geometry, origin.x, origin.y, stroke_width, page)
        .map_err(|_| Refusal::Failed("The connector does not fit the page"))?;
    let mut style = request
        .style
        .unwrap_or_else(|| if supplied_pen { pen.style() } else { json!({}) });
    let style_fields = style
        .as_object_mut()
        .ok_or(Refusal::Invalid("Invalid style"))?;
    style_fields.remove("startCap");
    style_fields.remove("endCap");
    arrow.apply_caps(&mut style);
    if !caps_fit(&shape, &style, stroke_width, page) {
        return Err(Refusal::Failed("The connector caps do not fit the page"));
    }
    Ok(batch(vec![json!({
        "op": "addShape",
        "x": origin.x,
        "y": origin.y,
        "geometry": geometry,
        "style": style,
        "addonData": {ADDON_ID: data.entry()},
    })]))
}

const CANNOT_FIT: Refusal = Refusal::Failed("The connector does not fit the page");
const REVIEW_LEGACY: Refusal = Refusal::Failed("Legacy connector upgrade requires review");
const BAD_SOURCE: Refusal = Refusal::Invalid("Invalid connector source");

struct Source {
    id: String,
    origin: Point,
    sin: f64,
    cos: f64,
    scale: f64,
    stroke: f64,
    geometry: Value,
    style: Value,
}

impl Source {
    fn read(value: &Value) -> Option<Self> {
        let number = |key: &str, default: f64| match value.get(key) {
            None => Some(default),
            Some(v) => v.as_f64().filter(|v| v.is_finite()),
        };
        if value["type"] != "shape" {
            return None;
        }
        let style = value.get("style")?.clone();
        let stroke = style["strokeWidthPt"].as_f64().unwrap_or(DEFAULT_WIDTH);
        let scale = number("scale", 1.0)?;
        if !style.is_object() || scale <= 0.0 || !(stroke * scale).is_finite() || stroke <= 0.0 {
            return None;
        }
        let (sin, cos) = number("rotation", 0.0)?.to_radians().sin_cos();
        Some(Self {
            id: value["id"].as_str()?.to_owned(),
            origin: Point::new(number("x", 0.0)?, number("y", 0.0)?),
            sin,
            cos,
            scale,
            stroke,
            geometry: value.get("geometry")?.clone(),
            style,
        })
    }

    fn to_page(&self, point: Point) -> Point {
        self.origin.plus(
            Point::new(
                point.x * self.cos - point.y * self.sin,
                point.x * self.sin + point.y * self.cos,
            )
            .scale(self.scale),
        )
    }

    fn inverse_vector(&self, vector: Point) -> Point {
        Point::new(
            vector.x * self.cos + vector.y * self.sin,
            -vector.x * self.sin + vector.y * self.cos,
        )
        .scale(1.0 / self.scale)
    }

    fn endpoints(&self, stored: &Stored, page: Page) -> Result<[Point; 2], Refusal> {
        let local = if let Some(arrow) = stored.legacy {
            legacy_tips(&stored.data, arrow, self.stroke, page, &self.geometry)
                .ok_or(REVIEW_LEGACY)?
        } else {
            let nodes = read_nodes(&self.geometry).map_err(invalid)?;
            [nodes[0].point, nodes[nodes.len() - 1].point]
        };
        Ok(local.map(|p| self.to_page(p)))
    }

    fn update(&self, stored: &mut Stored, page: Page) -> Result<Value, Refusal> {
        let data = &mut stored.data;
        let endpoints = [data.start_point, data.end_point];
        let stroke = self.stroke * self.scale;
        let (shape, origin) = build(data, stroke, page).ok_or(CANNOT_FIT)?;
        let nodes = shape.nodes();
        if nodes[0].point != endpoints[0] || nodes[nodes.len() - 1].point != endpoints[1] {
            return Err(CANNOT_FIT);
        }
        let mut style = self.style.clone();
        if let Some(arrow) = stored.legacy {
            arrow.apply_caps(&mut style);
        }
        if !caps_fit(&shape, &style, stroke, page) {
            return Err(CANNOT_FIT);
        }
        validate(&shape.geometry(origin), origin.x, origin.y, stroke, page)
            .map_err(|_| CANNOT_FIT)?;
        let local = |p: Point| self.inverse_vector(p.minus(self.origin));
        let shape = match shape {
            Shape::Line { start, end } => Shape::Line {
                start: local(start),
                end: local(end),
            },
            Shape::Spline { mut nodes } => {
                for node in &mut nodes {
                    node.point = local(node.point);
                    node.handle_in = node.handle_in.map(|h| self.inverse_vector(h));
                    node.handle_out = node.handle_out.map(|h| self.inverse_vector(h));
                }
                Shape::Spline { nodes }
            }
        };
        data.origin = self.origin;
        let mut op = json!({"op":"updateObject", "id":self.id,
            "geometry":shape.geometry(Point::default()), "addonData":data.entry()});
        if style != self.style {
            op["style"] = style;
        }
        Ok(op)
    }
}

struct Stored {
    data: Data,
    legacy: Option<Arrow>,
    refs: Vec<String>,
}

impl Stored {
    fn read(entry: &Value) -> Option<Self> {
        let (mut data, legacy) = Data::decode(entry.get("data")?.clone())?;
        let previous_refs = data.refs();
        let refs: Vec<String> = serde_json::from_value(entry.get("refs")?.clone()).ok()?;
        // Duplication remaps the ordered refs while leaving the addon payload opaque.
        if refs.len() == previous_refs.len() {
            for attachment in [&mut data.start, &mut data.end].into_iter().flatten() {
                let index = previous_refs
                    .iter()
                    .position(|id| id == &attachment.object_id)?;
                attachment.object_id.clone_from(&refs[index]);
            }
        }
        Some(Self { data, legacy, refs })
    }

    fn new(routing: Routing) -> Self {
        Self {
            data: Data {
                version: 2,
                routing,
                start: None,
                end: None,
                start_point: Point::default(),
                end_point: Point::default(),
                bends: Vec::new(),
                start_side: None,
                end_side: None,
                start_bounds: None,
                end_bounds: None,
                origin: Point::default(),
            },
            legacy: None,
            refs: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum PathEnd {
    Start,
    End,
}

impl PathEnd {
    fn index(self) -> usize {
        usize::from(self == Self::End)
    }
}

struct Endpoint<'a> {
    attachment: &'a mut Option<Attachment>,
    point: &'a mut Point,
    side: &'a mut Option<Side>,
    bounds: &'a mut Option<Rect>,
}

fn endpoints(data: &mut Data) -> [Endpoint<'_>; 2] {
    [
        Endpoint {
            attachment: &mut data.start,
            point: &mut data.start_point,
            side: &mut data.start_side,
            bounds: &mut data.start_bounds,
        },
        Endpoint {
            attachment: &mut data.end,
            point: &mut data.end_point,
            side: &mut data.end_side,
            bounds: &mut data.end_bounds,
        },
    ]
}

impl Endpoint<'_> {
    fn free(&mut self, point: Point) {
        *self.attachment = None;
        *self.point = point;
        *self.side = None;
        *self.bounds = None;
    }

    fn follow(
        &mut self,
        attachment: Attachment,
        (point, side, bounds): (Point, Option<Side>, Rect),
    ) {
        *self.attachment = Some(attachment);
        *self.point = point;
        *self.side = side;
        *self.bounds = Some(bounds);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Changed {
    page: PageParams,
    changed: Vec<Value>,
    deleted: Vec<String>,
    own: Vec<Value>,
}

/// Follow changed parts and release missing ports using their last confirmed locations.
pub fn page_changed(params: Value) -> Outcome {
    let request: Changed = serde_json::from_value(params).map_err(invalid)?;
    let page = request.page.page()?;
    let parts = snapshots(request.changed)?;
    let deleted: HashSet<String> = request.deleted.into_iter().collect();
    let mut operations = Vec::new();
    for value in request.own {
        let Some(mut stored) = value
            .pointer("/addonData/connectors")
            .and_then(Stored::read)
        else {
            continue;
        };
        if !stored
            .data
            .refs()
            .iter()
            .any(|id| parts.contains_key(id) || deleted.contains(id))
        {
            continue;
        }
        let source = Source::read(&value).ok_or(BAD_SOURCE)?;
        let tips = source.endpoints(&stored, page)?;
        let before = stored.data.clone();
        for (mut end, tip) in endpoints(&mut stored.data).into_iter().zip(tips) {
            let Some(attachment) = end.attachment.clone() else {
                end.free(tip);
                continue;
            };
            let id = &attachment.object_id;
            if deleted.contains(id) || !stored.refs.contains(id) {
                end.free(*end.point);
            } else if parts.contains_key(id) {
                if let Some(port) = locate(&attachment, &parts) {
                    end.follow(attachment, port);
                } else {
                    end.free(*end.point);
                }
            }
        }
        if stored.data == before && stored.legacy.is_none() {
            continue;
        }
        operations.push(source.update(&mut stored, page)?);
        if operations.len() == MAX_OPERATIONS {
            break;
        }
    }
    Ok(batch(operations))
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Edit {
    Endpoint { end: PathEnd },
    Choice { value: Routing },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathEdit {
    command: String,
    settings: Map<String, Value>,
    page: PageParams,
    edit: Edit,
    object: Value,
    points: [ToolPoint; 2],
    refs: Vec<Value>,
}

pub fn command_invoke(params: Value) -> Outcome {
    let request: PathEdit = serde_json::from_value(params).map_err(invalid)?;
    if request.command != COMMAND {
        return Err(Refusal::Invalid("Unknown command"));
    }
    let page = request.page.page()?;
    let (default_routing, _) = settings(&request.settings)?;
    let source = Source::read(&request.object).ok_or(BAD_SOURCE)?;
    let edited = match request.edit {
        Edit::Endpoint { end } => Some(end.index()),
        Edit::Choice { .. } => None,
    };
    let mut stored = match request.object.get("addonData") {
        None | Some(Value::Null) => {
            let capped = ["startCap", "endCap"].iter().any(|key| {
                matches!(
                    source.style[key].as_str(),
                    Some("arrow" | "open" | "dot" | "bar")
                )
            });
            if !capped || edited.is_some_and(|i| request.points[i].attach.is_none()) {
                return Err(BAD_SOURCE);
            }
            Stored::new(default_routing)
        }
        Some(entries) => {
            Stored::read(entries.get(ADDON_ID).ok_or(BAD_SOURCE)?).ok_or(BAD_SOURCE)?
        }
    };
    let canonical = source.endpoints(&stored, page)?;
    if let Edit::Choice { value } = request.edit {
        stored.data.routing = value;
        stored.data.bends.clear();
    }
    let parts = snapshots(request.refs)?;
    let legacy = stored.legacy.is_some();
    for (index, ((mut end, input), tip)) in endpoints(&mut stored.data)
        .into_iter()
        .zip(request.points)
        .zip(canonical)
        .enumerate()
    {
        let point = Point::new(input.x, input.y);
        if !point.finite() || (edited != Some(index) && input.attach.is_some()) {
            return Err(Refusal::Invalid("Invalid endpoint"));
        }
        if edited == Some(index) {
            if let Some(attachment) = input.attach {
                let port = locate(&attachment, &parts).ok_or(Refusal::Invalid("Unknown port"))?;
                if attachment.object_id == source.id || !port.0.near(point) {
                    return Err(Refusal::Invalid("Invalid snapped endpoint"));
                }
                end.follow(attachment, port);
            } else {
                end.free(point);
            }
        } else if let Some((attachment, port)) = end
            .attachment
            .clone()
            .filter(|a| stored.refs.contains(&a.object_id))
            .and_then(|a| locate(&a, &parts).map(|p| (a, p)))
        {
            end.follow(attachment, port);
        } else {
            end.free(if end.attachment.is_some() {
                *end.point
            } else if legacy {
                tip
            } else {
                point
            });
        }
    }
    Ok(batch(vec![source.update(&mut stored, page)?]))
}
