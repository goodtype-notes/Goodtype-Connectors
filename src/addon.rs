//! The three requests that create and maintain connectors: `tool/apply`, `page/changed` and the
//! `change-routing` own-object command. Each returns a `page-batch-v0` result.
use crate::{
    connector::{ADDON_ID, Arrow, Attachment, DEFAULT_WIDTH, Data, Page, Pen, build, validate},
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
    /// Not in the 3c contract yet: the active pen, so a connector matches it. Defaults otherwise.
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
        arrow,
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
    let (shape, origin) = build(&mut data, pen.stroke_width_pt, page)
        .ok_or(Refusal::Failed("The connector ends coincide"))?;
    let geometry = shape.geometry(origin);
    validate(&geometry, origin.x, origin.y, pen.stroke_width_pt, page)
        .map_err(|_| Refusal::Failed("The connector does not fit the page"))?;
    Ok(batch(vec![json!({
        "op": "addShape",
        "x": origin.x,
        "y": origin.y,
        "geometry": geometry,
        "style": if supplied_pen { pen.style() } else { json!({}) },
        "addonData": data.entry(),
    })]))
}

/// The fields of an own connector object the addon needs to re-route it.
struct Own {
    id: String,
    x: f64,
    y: f64,
    rotation: f64,
    scale: f64,
    stroke: f64,
    geometry: Value,
    data: Data,
}

fn own(value: &Value) -> Option<Own> {
    let number = |key: &str, default: f64| match value.get(key) {
        None => Some(default),
        Some(v) => v.as_f64().filter(|v| v.is_finite()),
    };
    let entry = value.get("addonData")?.get(ADDON_ID)?;
    let mut data: Data = serde_json::from_value(entry.get("data")?.clone()).ok()?;
    let previous_refs = data.refs();
    let refs: Vec<String> = serde_json::from_value(entry.get("refs")?.clone()).ok()?;
    // The host remaps refs on copy, while the opaque attachment data still has the old IDs.
    // A shorter list comes from deletion; follow() handles it with the deleted IDs.
    if refs.len() == previous_refs.len() {
        for attachment in [&mut data.start, &mut data.end].into_iter().flatten() {
            if let Some(index) = previous_refs
                .iter()
                .position(|id| id == &attachment.object_id)
            {
                attachment.object_id.clone_from(&refs[index]);
            }
        }
    }
    Some(Own {
        id: value.get("id")?.as_str()?.to_owned(),
        x: number("x", 0.0)?,
        y: number("y", 0.0)?,
        rotation: number("rotation", 0.0)?,
        scale: number("scale", 1.0)?,
        stroke: value
            .pointer("/style/strokeWidthPt")
            .and_then(Value::as_f64)
            .filter(|w| w.is_finite() && *w > 0.0)
            .unwrap_or(DEFAULT_WIDTH),
        geometry: value.get("geometry").cloned().unwrap_or(Value::Null),
        data,
    })
}

/// Bring one end up to date: follow its part, or let go when the part was deleted.
fn follow(
    attachment: &mut Option<Attachment>,
    point: &mut Point,
    side: &mut Option<Side>,
    bounds: &mut Option<Rect>,
    parts: &HashMap<String, Snapshot>,
    deleted: &HashSet<String>,
    shift: Point,
) {
    let Some(current) = attachment else {
        *point = point.plus(shift);
        return;
    };
    if deleted.contains(&current.object_id) {
        // Keep the last position; the user's connector survives as a free end.
        *attachment = None;
        *side = None;
        *bounds = None;
        return;
    }
    if parts.contains_key(&current.object_id) {
        match locate(current, parts) {
            Some((port, new_side, new_bounds)) => {
                *point = port;
                *side = new_side;
                *bounds = Some(new_bounds);
            }
            None => {
                // The part no longer has that port: keep the end where it was, unattached.
                *attachment = None;
                *side = None;
                *bounds = None;
            }
        }
    }
}

/// Re-route one own connector; `None` when nothing would change.
fn reroute(
    value: &Value,
    parts: &HashMap<String, Snapshot>,
    deleted: &HashSet<String>,
    page: Page,
    routing: Option<Routing>,
) -> Option<Value> {
    let Own {
        id,
        x,
        y,
        rotation,
        scale,
        stroke,
        geometry,
        mut data,
    } = own(value)?;
    let before = data.clone();
    let shift = if rotation == 0.0 && scale == 1.0 {
        Point::new(x, y).minus(data.origin)
    } else {
        Point::default()
    };
    follow(
        &mut data.start,
        &mut data.start_point,
        &mut data.start_side,
        &mut data.start_bounds,
        parts,
        deleted,
        shift,
    );
    follow(
        &mut data.end,
        &mut data.end_point,
        &mut data.end_side,
        &mut data.end_bounds,
        parts,
        deleted,
        shift,
    );
    if let Some(routing) = routing {
        data.routing = routing;
        data.bends.clear();
    }
    let mut op = json!({"op": "updateObject", "id": id});
    match build(&mut data, stroke, page) {
        Some((shape, origin)) => {
            let new_geometry = shape.geometry(origin);
            validate(&new_geometry, origin.x, origin.y, stroke, page).ok()?;
            let unchanged = new_geometry == geometry
                && origin == Point::new(x, y)
                && rotation == 0.0
                && scale == 1.0
                && data == before;
            if unchanged {
                return None;
            }
            op["x"] = json!(origin.x);
            op["y"] = json!(origin.y);
            if rotation != 0.0 {
                op["rotation"] = json!(0.0);
            }
            if scale != 1.0 {
                op["scale"] = json!(1.0);
            }
            op["geometry"] = new_geometry;
        }
        // Both ends now coincide: leave the drawing alone, but record any detachment.
        None if data != before => {}
        None => return None,
    }
    op["addonData"] = data.entry();
    Some(op)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Changed {
    page: PageParams,
    changed: Vec<Value>,
    deleted: Vec<String>,
    own: Vec<Value>,
}

/// `page/changed`: follow moved parts, release ends whose part was deleted. Never deletes.
pub fn page_changed(params: Value) -> Outcome {
    let request: Changed = serde_json::from_value(params).map_err(invalid)?;
    let page = request.page.page()?;
    let parts = snapshots(request.changed)?;
    let deleted: HashSet<String> = request.deleted.into_iter().collect();
    let operations: Vec<Value> = request
        .own
        .iter()
        .filter_map(|value| reroute(value, &parts, &deleted, page, None))
        .take(MAX_OPERATIONS)
        .collect();
    Ok(batch(operations))
}

/// `command/invoke` for `change-routing`: the selected connectors all take the routing after
/// the first one's (straight, curved, orthogonal, then straight again).
pub fn command_invoke(params: Value) -> Outcome {
    let Value::Object(mut params) = params else {
        return Err(Refusal::Invalid("Invalid command parameters"));
    };
    // The contract names `objects`; `scope` mirrors the selection commands' envelope.
    if let Some(Value::Object(scope)) = params.remove("scope") {
        for (key, value) in scope {
            params.entry(key).or_insert(value);
        }
    }
    let allowed = ["command", "settings", "page", "objects", "refs", "kind"];
    if params.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Refusal::Invalid("Unknown command parameter"));
    }
    if params.get("command").and_then(Value::as_str) != Some(COMMAND) {
        return Err(Refusal::Invalid("Unknown command"));
    }
    let page: PageParams = serde_json::from_value(
        params
            .remove("page")
            .ok_or(Refusal::Invalid("Missing page"))?,
    )
    .map_err(invalid)?;
    let page = page.page()?;
    let Some(Value::Array(objects)) = params.remove("objects") else {
        return Err(Refusal::Invalid("Missing objects"));
    };
    let (owned, mut others): (Vec<Value>, Vec<Value>) = objects
        .into_iter()
        .partition(|value| value.pointer(&format!("/addonData/{ADDON_ID}")).is_some());
    if let Some(Value::Array(refs)) = params.remove("refs") {
        others.extend(refs);
    }
    let parts = snapshots(others)?;
    let first = owned
        .iter()
        .find_map(own)
        .ok_or(Refusal::Invalid("No connectors selected"))?;
    let next = first.data.routing.next();
    let operations = owned
        .iter()
        .filter_map(|value| reroute(value, &parts, &HashSet::new(), page, Some(next)))
        .take(MAX_OPERATIONS)
        .collect();
    Ok(batch(operations))
}
