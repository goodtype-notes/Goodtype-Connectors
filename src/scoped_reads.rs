//! Invocation-local geometry reads. A failed or incomplete read keeps the existing router.
use crate::{
    connector::Data,
    geometry::Rect,
    route::{self, CLEARANCE, Route, Routing},
};
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn obstacles(
    params: &Value,
    data: &Data,
    mut read: impl FnMut(&str, Value) -> Option<Value>,
) -> Option<Vec<Rect>> {
    if data.routing != Routing::Orthogonal {
        return None;
    }
    let offer = params.get("reads")?;
    let revision = offer["revision"].as_u64()?;
    let allowed: Rect = serde_json::from_value(offer["region"].clone()).ok()?;
    if !allowed.valid() {
        return None;
    }
    let side = offer["maxRegionSidePt"].as_f64()?.min(1024.0);
    let refs = data.refs();
    let groups: Vec<_> = offer["groups"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter(|id| refs.iter().any(|r| r == id))
        .collect();
    // Two distinct endpoint groups need three reads; keep each commit to two at most.
    if groups.len() > 1 || offer["maxCalls"].as_u64()? < (1 + groups.len()) as u64 {
        return None;
    }
    let (start, end) = data.ends();
    let Route::Polyline(points) = route::route(data.routing, start, end, &data.bends).route else {
        return None;
    };
    let mut bounds = Rect {
        x: f64::INFINITY,
        y: f64::INFINITY,
        width: 0.0,
        height: 0.0,
    };
    let (mut right, mut bottom) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in points {
        bounds.x = bounds.x.min(p.x);
        bounds.y = bounds.y.min(p.y);
        right = right.max(p.x);
        bottom = bottom.max(p.y);
    }
    bounds.x = (bounds.x - CLEARANCE).max(allowed.x);
    bounds.y = (bounds.y - CLEARANCE).max(allowed.y);
    bounds.width = (right + CLEARANCE).min(allowed.right()) - bounds.x;
    bounds.height = (bottom + CLEARANCE).min(allowed.bottom()) - bounds.y;
    // A truncated long corridor is not evidence that the rest is clear.
    if !bounds.valid()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
        || bounds.width > side
        || bounds.height > side
    {
        return None;
    }
    let mut excluded: HashSet<String> = refs.into_iter().collect();
    if let Some(id) = params.pointer("/object/id").and_then(Value::as_str) {
        excluded.insert(id.into());
    }
    let mut bytes = 0;
    let mut query = |method: &str, args: Value| -> Option<Vec<Value>> {
        let result = read(method, args)?;
        let size = serde_json::to_vec(&result).ok()?.len();
        bytes += size;
        let objects = result["objects"].as_array()?;
        if result["revision"].as_u64()? != revision
            || size > 65536
            || size as u64 > offer["maxBytes"].as_u64()?
            || bytes as u64 > offer["maxTotalBytes"].as_u64()?
            || bytes > 262144
            || objects.len() > 256
            || objects.len() as u64 > offer["maxObjects"].as_u64()?
        {
            return None;
        }
        Some(objects.clone())
    };
    for group in groups {
        for object in query(
            "group/descendants",
            json!({"revision":revision,"groupId":group}),
        )? {
            excluded.insert(object["id"].as_str()?.into());
        }
    }
    let objects = query(
        "region/obstacles",
        json!({"revision":revision,"bounds":bounds}),
    )?;
    filter(&objects, &excluded)
}

fn filter(objects: &[Value], excluded: &HashSet<String>) -> Option<Vec<Rect>> {
    let mut boxes = Vec::new();
    for object in objects {
        if excluded.contains(object["id"].as_str()?) {
            continue;
        }
        let bounds: Rect = serde_json::from_value(object["bounds"].clone()).ok()?;
        if !bounds.valid() {
            return None;
        }
        boxes.push(bounds.inflate(CLEARANCE));
    }
    // Bound the existing Cartesian-grid search, whose work grows with both axes.
    (boxes.len() <= 32).then_some(boxes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_only_own_path_endpoints_and_explicit_members() {
        let objects: Vec<_> = ["own", "a", "b", "child", "unrelated"]
            .iter()
            .map(|id| json!({"id":id,"bounds":{"x":10,"y":20,"width":30,"height":40}}))
            .collect();
        let excluded = ["own", "a", "b", "child"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            filter(&objects, &excluded),
            Some(vec![Rect {
                x: -2.0,
                y: 8.0,
                width: 54.0,
                height: 64.0
            }])
        );
        assert!(filter(&vec![objects[4].clone(); 33], &excluded).is_none());
        assert!(filter(&[json!({"id":"bad"})], &excluded).is_none());
    }
}
