//! Protocol round trips against a fake host that applies replies the way the Step 3c contract
//! describes: create ops gain an ID, `updateObject` and `deleteObject` may only touch objects
//! carrying this addon's entry, and every shape passes core-equivalent validation.
use goodtype_connectors::{
    connector::{Page, validate},
    session,
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, io::Cursor};

const A4: Page = Page {
    width: 595.28,
    height: 841.89,
};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Replace each `"OWN"` placeholder with the given own object.
fn with_own(value: Value, own: &Value) -> Value {
    match value {
        Value::String(s) if s == "OWN" => own.clone(),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| with_own(v, own)).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, with_own(v, own)))
                .collect(),
        ),
        other => other,
    }
}

/// Run one session: the initialize fixture, then `frames`. Returns the replies after the
/// handshake, which must have succeeded.
fn exchange(frames: &[Value]) -> Vec<Value> {
    let mut input = format!("{}\n", fixture("initialize"));
    for frame in frames {
        input.push_str(&format!("{frame}\n"));
    }
    let mut output = Vec::new();
    session::run(Cursor::new(input), &mut output).expect("session stays valid");
    let mut rows: Vec<Value> = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let handshake = rows.remove(0);
    assert_eq!(
        handshake["result"]["capabilities"],
        json!(["page-batch-v0", "path-edit-v0"])
    );
    for (row, frame) in rows.iter().zip(frames) {
        assert_eq!(row["id"], frame["id"]);
        assert_eq!(row["jsonrpc"], "2.0");
    }
    rows
}

fn one(frame: Value) -> Value {
    exchange(&[frame]).remove(0)
}

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn keys(value: &Value, required: &[&str], optional: &[&str]) -> Result<(), String> {
    let map = value.as_object().ok_or("expected an object")?;
    for key in required {
        if !map.contains_key(*key) {
            return Err(format!("missing {key}"));
        }
    }
    for key in map.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(format!("unknown field {key}"));
        }
    }
    Ok(())
}

struct FakeHost {
    objects: BTreeMap<String, Value>,
    created: usize,
}

impl FakeHost {
    fn new(parts: &[Value]) -> Self {
        Self {
            objects: parts
                .iter()
                .map(|p| (p["id"].as_str().unwrap().to_owned(), p.clone()))
                .collect(),
            created: 0,
        }
    }

    fn own(&self, id: &str) -> bool {
        self.objects
            .get(id)
            .is_some_and(|o| o.pointer("/addonData/connectors").is_some())
    }

    fn check_entry(&self, id: &str, entry: &Value) -> Result<(), String> {
        keys(entry, &["refs", "data"], &[])?;
        let refs = entry["refs"].as_array().ok_or("refs must be a list")?;
        let mut seen = Vec::new();
        for r in refs {
            let r = r.as_str().ok_or("ref must be a string")?;
            if r == id || seen.contains(&r) || !self.objects.contains_key(r) {
                return Err(format!("bad ref {r}"));
            }
            seen.push(r);
        }
        if refs.len() > 32
            || serde_json::to_string(&entry["data"]).unwrap().len() > 16 * 1024
            || depth(&entry["data"]) > 8
        {
            return Err("addon data exceeds its limits".into());
        }
        Ok(())
    }

    fn check_style(style: &Value) -> Result<f64, String> {
        keys(
            style,
            &[],
            &[
                "strokeColor",
                "strokeWidthPt",
                "fillColor",
                "opacity",
                "pattern",
                "patternSpacing",
                "patternOffsetPt",
                "startCap",
                "endCap",
            ],
        )?;
        if style.get("strokeWidthPt").is_none() {
            return Ok(1.6);
        }
        style["strokeWidthPt"]
            .as_f64()
            .filter(|w| *w > 0.0 && *w <= 256.0)
            .ok_or_else(|| "bad stroke width".into())
    }

    /// Validate the whole batch first, then apply it; any invalid op rejects everything.
    fn apply(&mut self, result: &Value) -> Result<Vec<String>, String> {
        keys(result, &["batch"], &[])?;
        keys(&result["batch"], &["operations"], &[])?;
        let operations = result["batch"]["operations"].as_array().unwrap();
        if operations.len() > 32 {
            return Err("too many operations".into());
        }
        let mut staged = self.objects.clone();
        let mut touched = Vec::new();
        for op in operations {
            match op["op"].as_str() {
                Some("addShape") => {
                    keys(
                        op,
                        &["op", "x", "y", "geometry", "style"],
                        &["addonData", "ports"],
                    )?;
                    let width = Self::check_style(&op["style"])?;
                    let (x, y) = (op["x"].as_f64().unwrap(), op["y"].as_f64().unwrap());
                    validate(&op["geometry"], x, y, width, A4)?;
                    self.created += 1;
                    let id = format!("addon-{}", self.created);
                    if let Some(entry) = op.get("addonData") {
                        self.check_entry(&id, &entry["connectors"])?;
                    }
                    let mut object = Map::new();
                    object.insert("type".into(), json!("shape"));
                    object.insert("id".into(), json!(id));
                    for (k, v) in op.as_object().unwrap() {
                        if k != "op" {
                            object.insert(k.clone(), v.clone());
                        }
                    }
                    object.insert("rotation".into(), json!(0.0));
                    object.insert("scale".into(), json!(1.0));
                    staged.insert(id.clone(), Value::Object(object));
                    touched.push(id);
                }
                Some("updateObject") => {
                    keys(
                        op,
                        &["op", "id"],
                        &[
                            "x",
                            "y",
                            "rotation",
                            "scale",
                            "geometry",
                            "style",
                            "source",
                            "ports",
                            "addonData",
                        ],
                    )?;
                    let id = op["id"].as_str().unwrap();
                    if !self.own(id) {
                        return Err(format!("{id} does not carry this addon's entry"));
                    }
                    let object = staged.get_mut(id).unwrap();
                    for (k, v) in op.as_object().unwrap() {
                        if k == "addonData" {
                            object["addonData"]["connectors"] = v.clone();
                        } else if k != "op" && k != "id" {
                            object[k] = v.clone();
                        }
                    }
                    let width = Self::check_style(&object["style"])?;
                    let (x, y) = (object["x"].as_f64().unwrap(), object["y"].as_f64().unwrap());
                    validate(&object["geometry"], x, y, width, A4)?;
                    if let Some(entry) = op.get("addonData") {
                        self.check_entry(id, entry)?;
                    }
                    touched.push(id.to_owned());
                }
                Some("deleteObject") => {
                    let id = op["id"].as_str().unwrap();
                    if !self.own(id) {
                        return Err(format!("{id} does not carry this addon's entry"));
                    }
                    staged.remove(id);
                    touched.push(id.to_owned());
                }
                _ => return Err(format!("unsupported op {op}")),
            }
        }
        self.objects = staged;
        Ok(touched)
    }

    /// The connector's path in page coordinates (spline nodes or line ends).
    fn path(&self, id: &str) -> Vec<(f64, f64)> {
        let object = &self.objects[id];
        let (x, y) = (object["x"].as_f64().unwrap(), object["y"].as_f64().unwrap());
        let at = |p: &Value| (x + p["x"].as_f64().unwrap(), y + p["y"].as_f64().unwrap());
        let geometry = &object["geometry"];
        match geometry["kind"].as_str().unwrap() {
            "line" => vec![at(&geometry["start"]), at(&geometry["end"])],
            _ => geometry["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| at(&n["point"]))
                .collect(),
        }
    }

    /// Move a part: shift its position, bounds and ports, returning its new snapshot.
    fn move_part(&mut self, id: &str, dx: f64, dy: f64) -> Value {
        let part = self.objects.get_mut(id).unwrap();
        for (key, delta) in [("x", dx), ("y", dy)] {
            part[key] = json!(part[key].as_f64().unwrap() + delta);
            part["bounds"][key] = json!(part["bounds"][key].as_f64().unwrap() + delta);
        }
        for port in part["ports"].as_array_mut().unwrap() {
            port["x"] = json!(port["x"].as_f64().unwrap() + dx);
            port["y"] = json!(port["y"].as_f64().unwrap() + dy);
        }
        part.clone()
    }
}

fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
}

fn assert_orthogonal(path: &[(f64, f64)]) {
    for w in path.windows(2) {
        assert!(
            (w[0].0 - w[1].0).abs() < 1e-6 || (w[0].1 - w[1].1).abs() < 1e-6,
            "diagonal run in {path:?}"
        );
        assert!(!near(w[0], w[1]), "zero-length run in {path:?}");
    }
}

fn parts_from_tool_fixture() -> Vec<Value> {
    fixture("tool-apply")["params"]["objects"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn tool_apply_creates_one_valid_attached_connector() {
    let reply = one(fixture("tool-apply"));
    let result = &reply["result"];
    let mut host = FakeHost::new(&parts_from_tool_fixture());
    let created = host.apply(result).unwrap();
    assert_eq!(created, ["addon-1"]);
    let op = &result["batch"]["operations"][0];
    assert_eq!(op["op"], "addShape");
    assert_eq!(op["style"], json!({"endCap": "open"}));
    let entry = &op["addonData"]["connectors"];
    assert_eq!(entry["refs"], json!(["shape-r1", "shape-c2"]));
    assert_eq!(entry["data"]["routing"], "orthogonal");
    assert_eq!(entry["data"]["version"], 2);
    assert!(entry["data"].get("arrow").is_none());
    assert_eq!(
        entry["data"]["start"],
        json!({"objectId": "shape-r1", "port": 1})
    );
    assert_eq!(
        entry["data"]["end"],
        json!({"objectId": "shape-c2", "port": 0})
    );
    let route = &host.path("addon-1");
    assert!(near(route[0], (148.0, 108.0)));
    assert!(near(*route.last().unwrap(), (300.0, 172.0)));
    assert_orthogonal(route);
    assert_eq!(
        route.as_slice(),
        [
            (148.0, 108.0),
            (224.0, 108.0),
            (224.0, 172.0),
            (300.0, 172.0)
        ]
    );
}

#[test]
fn each_routing_and_arrow_setting_produces_a_valid_shape() {
    for routing in ["straight", "curved", "orthogonal"] {
        for arrow in ["none", "end", "both"] {
            let mut frame = fixture("tool-apply");
            frame["params"]["settings"] = json!({"routing": routing, "arrow": arrow});
            let reply = one(frame);
            let mut host = FakeHost::new(&parts_from_tool_fixture());
            host.apply(&reply["result"])
                .unwrap_or_else(|e| panic!("{routing}/{arrow}: {e}"));
            let kind = reply["result"]["batch"]["operations"][0]["geometry"]["kind"].clone();
            let expected = if routing == "straight" {
                "line"
            } else {
                "spline"
            };
            assert_eq!(kind, expected, "{routing}/{arrow}");
        }
    }
}

#[test]
fn tool_apply_uses_the_hosts_pen_when_given() {
    let mut frame = fixture("tool-apply");
    frame["params"]["pen"] =
        json!({"strokeColor": "#1d4ed8", "strokeWidthPt": 3, "opacity": 0.8, "pattern": "dashed"});
    let reply = one(frame);
    assert_eq!(
        reply["result"]["batch"]["operations"][0]["style"],
        json!({"strokeColor": "#1d4ed8", "strokeWidthPt": 3.0, "fillColor": null,
            "opacity": 0.8, "pattern": "dashed", "endCap": "open"})
    );
}

#[test]
fn tool_apply_preserves_the_hosts_shape_style_with_arrowheads() {
    let mut frame = fixture("tool-apply");
    frame["params"]["settings"]["arrow"] = json!("both");
    let mut style = json!({"strokeColor": "#1d4ed8", "strokeWidthPt": 3.0, "fillColor": null,
        "opacity": 0.55, "pattern": "dotted", "patternSpacing": 1.5});
    frame["params"]["style"] = style.clone();
    let reply = one(frame);
    let op = &reply["result"]["batch"]["operations"][0];
    style["startCap"] = json!("open");
    style["endCap"] = json!("open");
    assert_eq!(op["style"], style);
    let mut host = FakeHost::new(&parts_from_tool_fixture());
    host.apply(&reply["result"]).unwrap();
    assert_eq!(host.path("addon-1").len(), 4);
}

#[test]
fn free_points_and_mixed_ends_connect() {
    let mut frame = fixture("tool-apply");
    frame["params"]["points"][1] = json!({"x": 400, "y": 400, "attach": null});
    frame["params"]["objects"] = json!([frame["params"]["objects"][0]]);
    let reply = one(frame);
    let mut host = FakeHost::new(&parts_from_tool_fixture());
    host.apply(&reply["result"]).unwrap();
    let entry = &reply["result"]["batch"]["operations"][0]["addonData"]["connectors"];
    assert_eq!(entry["refs"], json!(["shape-r1"]));
    assert_eq!(entry["data"]["end"], Value::Null);
    let path = host.path("addon-1");
    assert!(near(*path.last().unwrap(), (400.0, 400.0)));
}

#[test]
fn invalid_tool_requests_are_refused_without_ending_the_session() {
    let mut unknown_port = fixture("tool-apply");
    unknown_port["params"]["points"][0]["attach"]["port"] = json!(7);
    let mut coincide = fixture("tool-apply");
    coincide["params"]["points"] = json!([{"x": 10, "y": 10, "attach": null},
        {"x": 10, "y": 10, "attach": null}]);
    coincide["id"] = json!("3");
    let mut bad_setting = fixture("tool-apply");
    bad_setting["params"]["settings"]["routing"] = json!("diagonal");
    bad_setting["id"] = json!("4");
    let mut extra = fixture("tool-apply");
    extra["params"]["surprise"] = json!(true);
    extra["id"] = json!("5");
    let rows = exchange(&[unknown_port, coincide, bad_setting, extra, {
        let mut ok = fixture("tool-apply");
        ok["id"] = json!("6");
        ok
    }]);
    let codes: Vec<_> = rows.iter().map(|r| r["error"]["code"].clone()).collect();
    assert_eq!(
        codes[..4],
        [json!(-32602), json!(-32001), json!(-32602), json!(-32602)]
    );
    assert!(rows[4]["result"]["batch"].is_object());
}

#[test]
fn moved_part_reroutes_and_keeps_the_users_style() {
    let own = fixture("own-connector");
    let reply = one(with_own(fixture("page-changed-moved"), &own));
    let ops = reply["result"]["batch"]["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["op"], "updateObject");
    assert_eq!(ops[0]["id"], "addon-0f3c9a");
    assert!(ops[0].get("style").is_none(), "the user's colour stays");
    let mut host = FakeHost::new(&parts_from_tool_fixture());
    host.objects.insert("addon-0f3c9a".into(), own);
    host.move_part("shape-c2", 0.0, 80.0);
    host.apply(&reply["result"]).unwrap();
    let path = host.path("addon-0f3c9a");
    assert_orthogonal(&path);
    assert!(near(path[0], (148.0, 108.0)));
    assert!(near(*path.last().unwrap(), (300.0, 252.0)));
    let data = &ops[0]["addonData"]["data"];
    assert_eq!(data["endPoint"], json!({"x": 300.0, "y": 252.0}));
    assert_eq!(data["endBounds"]["y"], json!(240.0));
}

#[test]
fn deleted_part_releases_the_end_and_keeps_the_connector() {
    let own = fixture("own-connector");
    let reply = one(with_own(fixture("page-changed-deleted"), &own));
    let ops = reply["result"]["batch"]["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["op"], "updateObject", "never deleted");
    let entry = &ops[0]["addonData"];
    assert_eq!(entry["refs"], json!(["shape-r1"]));
    assert_eq!(entry["data"]["end"], Value::Null);
    assert_eq!(entry["data"]["endPoint"], json!({"x": 300.0, "y": 172.0}));
    assert_eq!(entry["data"]["endSide"], "left");
    assert!(entry["data"]["endBounds"].is_null());
    let mut host = FakeHost::new(&parts_from_tool_fixture());
    host.objects.remove("shape-c2");
    host.objects.insert("addon-0f3c9a".into(), own);
    host.apply(&reply["result"]).unwrap();
    let path = host.path("addon-0f3c9a");
    assert!(near(*path.last().unwrap(), (300.0, 172.0)));
    assert_orthogonal(&path);
}

#[test]
fn full_round_trip_tool_move_noop_and_delete() {
    let mut host = FakeHost::new(&parts_from_tool_fixture());
    let created = host.apply(&one(fixture("tool-apply"))["result"]).unwrap();
    let id = created[0].clone();

    // An unchanged part produces no amendment.
    let snapshot = host.objects["shape-r1"].clone();
    let frame = json!({"jsonrpc": "2.0", "id": "2", "method": "page/changed", "params": {
        "page": {"widthPt": A4.width, "heightPt": A4.height},
        "changed": [snapshot], "deleted": [], "own": [host.objects[&id].clone()]}});
    assert_eq!(one(frame)["result"], json!({"batch": {"operations": []}}));

    // Move the resistor up and left; the connector follows on release.
    let moved = host.move_part("shape-r1", -40.0, -30.0);
    let frame = json!({"jsonrpc": "2.0", "id": "3", "method": "page/changed", "params": {
        "page": {"widthPt": A4.width, "heightPt": A4.height},
        "changed": [moved], "deleted": [], "own": [host.objects[&id].clone()]}});
    host.apply(&one(frame)["result"]).unwrap();
    let path = host.path(&id);
    assert!(near(path[0], (108.0, 78.0)));
    assert_orthogonal(&path);

    // Delete the capacitor: the end stays where it was and becomes free.
    host.objects.remove("shape-c2");
    let frame = json!({"jsonrpc": "2.0", "id": "4", "method": "page/changed", "params": {
        "page": {"widthPt": A4.width, "heightPt": A4.height},
        "changed": [], "deleted": ["shape-c2"], "own": [host.objects[&id].clone()]}});
    host.apply(&one(frame)["result"]).unwrap();
    let path = host.path(&id);
    assert!(near(*path.last().unwrap(), (300.0, 172.0)));
    assert_eq!(
        host.objects[&id]["addonData"]["connectors"]["refs"],
        json!(["shape-r1"])
    );
}

#[test]
fn a_moved_connector_carries_its_free_ends_along() {
    let mut own = fixture("own-connector");
    let data = own["addonData"]["connectors"]["data"]
        .as_object_mut()
        .unwrap();
    data.remove("arrow");
    data.insert("version".into(), json!(2));
    own["addonData"]["connectors"]["data"]["end"] = Value::Null;
    own["addonData"]["connectors"]["data"]["endSide"] = Value::Null;
    own["addonData"]["connectors"]["data"]["endBounds"] = Value::Null;
    own["addonData"]["connectors"]["refs"] = json!(["shape-r1"]);
    // The user dragged the connector 10 pt right and 20 pt down.
    own["x"] = json!(158.0);
    own["y"] = json!(128.0);
    let frame = json!({"jsonrpc": "2.0", "id": "2", "method": "page/changed", "params": {
        "page": {"widthPt": A4.width, "heightPt": A4.height},
        "changed": [parts_from_tool_fixture()[0].clone()], "deleted": [], "own": [own]}});
    let reply = one(frame);
    let data = &reply["result"]["batch"]["operations"][0]["addonData"]["data"];
    assert_eq!(data["endPoint"], json!({"x": 310.0, "y": 192.0}));
    assert_eq!(data["startPoint"], json!({"x": 148.0, "y": 108.0}));
}

#[test]
fn reactions_ignore_objects_without_this_addons_data() {
    let mut stranger = fixture("own-connector");
    stranger["addonData"] = json!({"charts": {"refs": ["shape-c2"], "data": {}}});
    let reply = one(with_own(fixture("page-changed-moved"), &stranger));
    assert_eq!(reply["result"], json!({"batch": {"operations": []}}));
}

#[test]
fn a_copied_connector_follows_remapped_refs() {
    let mut request = fixture("page-changed-moved");
    let mut connector = fixture("own-connector");
    connector["addonData"]["connectors"]["refs"][1] = json!("shape-c2-copy");
    request["params"]["own"][0] = connector;
    request["params"]["changed"][0]["id"] = json!("shape-c2-copy");
    let reply = one(request);
    let entry = &reply["result"]["batch"]["operations"][0]["addonData"];
    assert_eq!(entry["refs"], json!(["shape-r1", "shape-c2-copy"]));
    assert_eq!(entry["data"]["end"]["objectId"], "shape-c2-copy");
    assert_eq!(entry["data"]["endPoint"], json!({"x": 300.0, "y": 252.0}));
}

#[test]
fn a_port_too_close_to_the_edge_cannot_shift_off_its_attachment() {
    let mut request = fixture("tool-apply");
    request["params"]["objects"][0]["ports"][1]["x"] = json!(0.0);
    let reply = one(request);
    assert_eq!(reply["error"]["code"], -32001);
}

#[test]
fn port_drop_matches_the_contract_reply() {
    assert_eq!(
        one(fixture("command-invoke")),
        json!({
            "jsonrpc":"2.0","id":"17","result":{"batch":{"operations":[{
                "op":"updateObject","id":"arrow-1",
                "geometry":{"kind":"spline","closed":false,"nodes":[
                    {"point":{"x":0.0,"y":0.0},"handleIn":null,"handleOut":null},
                    {"point":{"x":120.0,"y":0.0},"handleIn":null,"handleOut":null}]},
                "addonData":{"refs":["r1"],"data":{"version":2,"routing":"orthogonal",
                    "start":null,"end":{"objectId":"r1","port":0},"startPoint":{"x":40.0,"y":100.0},
                    "endPoint":{"x":160.0,"y":100.0},"bends":[],"startSide":"right","endSide":"left",
                    "endBounds":{"x":160.0,"y":90.0,"width":40.0,"height":20.0},
                    "origin":{"x":40.0,"y":100.0}}}
            }]}}
        })
    );
}

#[test]
fn a_routing_pick_adopts_an_open_spline_and_retains_transformed_endpoints() {
    for routing in ["straight", "curved", "orthogonal"] {
        let mut request = fixture("command-invoke");
        let params = &mut request["params"];
        params["edit"] = json!({"kind":"choice","value":routing});
        params["object"]["rotation"] = json!(90);
        params["object"]["scale"] = json!(2);
        params["object"]["geometry"] = json!({"kind":"spline","closed":false,"nodes":[
            {"point":{"x":0,"y":0},"handleIn":null,"handleOut":null},
            {"point":{"x":40,"y":-15},"handleIn":null,"handleOut":null},
            {"point":{"x":100,"y":0},"handleIn":null,"handleOut":null}]});
        params["points"][1] = json!({"x":40,"y":300,"attach":null});
        params["refs"] = json!([]);
        let reply = one(request);
        let op = &reply["result"]["batch"]["operations"][0];
        keys(op, &["op", "id", "geometry", "addonData"], &[]).unwrap();
        assert_eq!(op["addonData"]["refs"], json!([]));
        assert_eq!(op["addonData"]["data"]["routing"], routing);
        assert!(op["addonData"]["data"]["start"].is_null());
        assert!(op["addonData"]["data"]["end"].is_null());
        let nodes = goodtype_connectors::connector::read_nodes(&op["geometry"]).unwrap();
        assert!(near((nodes[0].point.x, nodes[0].point.y), (0.0, 0.0)));
        let end = nodes.last().unwrap().point;
        assert!(near((end.x, end.y), (100.0, 0.0)));
    }
}

#[test]
fn releasing_an_owned_end_keeps_the_other_attachment_and_routing() {
    let mut reversed = fixture("command-invoke");
    reversed["params"]["edit"]["end"] = json!("start");
    reversed["params"]["points"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    reversed["params"]["object"]["x"] = json!(160);
    reversed["params"]["object"]["geometry"]["end"]["x"] = json!(-120);
    let first = one(reversed.clone());
    let op = &first["result"]["batch"]["operations"][0];
    let params = &mut reversed["params"];
    params["object"]["geometry"] = op["geometry"].clone();
    params["object"]["addonData"] = json!({"connectors":op["addonData"]});
    params["settings"]["routing"] = json!("straight");
    params["edit"]["end"] = json!("end");
    params["points"][0]["attach"] = Value::Null;
    params["points"][1] = json!({"x":55,"y":120,"attach":null});
    let reply = one(reversed);
    let entry = &reply["result"]["batch"]["operations"][0]["addonData"];
    assert_eq!(entry["refs"], json!(["r1"]));
    assert_eq!(entry["data"]["start"]["objectId"], "r1");
    assert_eq!(entry["data"]["routing"], "orthogonal");
    assert_eq!(entry["data"]["endPoint"], json!({"x":55.0,"y":120.0}));

    let mut release = fixture("command-invoke");
    let adopted = one(release.clone());
    let op = &adopted["result"]["batch"]["operations"][0];
    release["params"]["object"]["geometry"] = op["geometry"].clone();
    release["params"]["object"]["addonData"] = json!({"connectors":op["addonData"]});
    release["params"]["points"][1] = json!({"x":150,"y":130,"attach":null});
    let reply = one(release);
    let entry = &reply["result"]["batch"]["operations"][0]["addonData"];
    assert_eq!(entry["refs"], json!([]));
    assert!(entry["data"]["end"].is_null());
    assert_eq!(entry["data"]["endPoint"], json!({"x":150.0,"y":130.0}));
}

fn legacy_arrow() -> Value {
    let mut request = fixture("command-invoke");
    let params = &mut request["params"];
    params["edit"] = json!({"kind":"choice","value":"straight"});
    params["refs"] = json!([]);
    params["points"] = json!([{"x":40,"y":100,"attach":null},{"x":130,"y":104.2,"attach":null}]);
    params["object"]["y"] = json!(95.8);
    params["object"]["style"]
        .as_object_mut()
        .unwrap()
        .remove("endCap");
    let nodes = [
        [0.0, 4.2],
        [100.0, 4.2],
        [90.0, 8.4],
        [100.0, 4.2],
        [90.0, 0.0],
    ]
    .map(|[x, y]| json!({"point":{"x":x,"y":y},"handleIn":null,"handleOut":null}));
    params["object"]["geometry"] = json!({"kind":"spline","closed":false,"nodes":nodes});
    params["object"]["addonData"] = json!({"connectors":{"refs":[],"data":{
        "routing":"straight","arrow":"end","start":null,"end":null,
        "startPoint":{"x":40,"y":100},"endPoint":{"x":140,"y":100},
        "origin":{"x":40,"y":95.8}
    }}});
    request
}

#[test]
fn legacy_wings_convert_from_the_verified_logical_tip() {
    let reply = one(legacy_arrow());
    let op = &reply["result"]["batch"]["operations"][0];
    assert_eq!(op["style"]["endCap"], "open");
    assert_eq!(op["addonData"]["data"]["version"], 2);
    assert_eq!(op["geometry"]["kind"], "line");
    assert_eq!(op["geometry"]["end"]["x"], 100.0);
}

#[test]
fn a_manually_edited_legacy_wing_requires_review_and_returns_no_batch() {
    let mut request = legacy_arrow();
    request["params"]["object"]["geometry"]["nodes"][4]["point"]["x"] = json!(89);
    let reply = one(request);
    assert_eq!(
        reply["error"]["message"],
        "Legacy connector upgrade requires review"
    );
    assert!(reply.get("result").is_none());
}

#[test]
fn a_legacy_connector_upgrades_in_its_current_transform() {
    let mut request = legacy_arrow();
    let object = &mut request["params"]["object"];
    object["x"] = json!(70);
    object["y"] = json!(150);
    object["rotation"] = json!(90);
    object["scale"] = json!(2);
    object["style"]["endCap"] = json!("dot");
    let reply = one(request);
    let op = &reply["result"]["batch"]["operations"][0];
    keys(op, &["op", "id", "geometry", "addonData"], &[]).unwrap();
    let nodes = goodtype_connectors::connector::read_nodes(&op["geometry"]).unwrap();
    assert!(near((nodes[0].point.x, nodes[0].point.y), (0.0, 4.2)));
    assert!(near((nodes[1].point.x, nodes[1].point.y), (100.0, 4.2)));
}

#[test]
fn a_cap_that_would_clip_refuses_the_exact_route() {
    let mut request = fixture("command-invoke");
    let params = &mut request["params"];
    params["object"]["style"]["endCap"] = json!("dot");
    params["points"][1]["y"] = json!(1.0);
    params["refs"][0]["ports"][0]["y"] = json!(1.0);
    params["refs"][0]["bounds"]["y"] = json!(1.0);
    let reply = one(request);
    assert_eq!(reply["error"]["code"], -32001);
    assert!(reply.get("result").is_none());
}

#[test]
fn a_missing_drop_port_returns_no_update() {
    let mut request = fixture("command-invoke");
    request["params"]["refs"][0]["ports"] = json!([]);
    let reply = one(request);
    assert_eq!(reply["error"]["code"], -32602);
    assert!(reply.get("result").is_none());
}

#[test]
fn curved_pack_ports_infer_and_cache_sides() {
    let mut frame = fixture("tool-apply");
    frame["params"]["settings"] = json!({"routing": "curved", "arrow": "none"});
    let reply = one(frame);
    let op = &reply["result"]["batch"]["operations"][0];
    let data = &op["addonData"]["connectors"]["data"];
    assert_eq!(data["startSide"], "right");
    assert_eq!(data["endSide"], "left");
    let nodes = op["geometry"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4);
    let out = &nodes[1]["handleOut"];
    let into = &nodes[2]["handleIn"];
    assert!(out["x"].as_f64().unwrap() > 0.0);
    assert!(into["x"].as_f64().unwrap() < 0.0);
    assert_eq!(out["y"], 0.0);
    assert_eq!(into["y"], 0.0);
    // The diagonal chord is not collinear with either horizontal port handle.
    assert_ne!(nodes[1]["point"]["y"], nodes[2]["point"]["y"]);
    FakeHost::new(&parts_from_tool_fixture())
        .apply(&reply["result"])
        .unwrap();
}

#[test]
fn mid_page_free_curve_has_straight_leads_after_fit() {
    let mut frame = fixture("tool-apply");
    frame["params"]["settings"] = json!({"routing": "curved", "arrow": "none"});
    frame["params"]["points"] = json!([{"x":100,"y":200},{"x":300,"y":200}]);
    frame["params"]["objects"] = json!([]);
    let reply = one(frame);
    let op = &reply["result"]["batch"]["operations"][0];
    let nodes = op["geometry"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4);
    assert!(nodes[0]["handleOut"].is_null());
    assert!(nodes[3]["handleIn"].is_null());
    assert_eq!(nodes[1]["point"]["x"], 12.0);
    assert_eq!(nodes[2]["point"]["x"], 188.0);
    assert_eq!(nodes[1]["handleOut"]["y"], 0.0);
    assert_eq!(nodes[2]["handleIn"]["y"], 0.0);
    FakeHost::new(&[]).apply(&reply["result"]).unwrap();
}

#[test]
fn repeated_free_endpoint_edits_keep_the_cached_axis_near_the_diagonal() {
    for routing in ["curved", "orthogonal"] {
        let mut request = fixture("command-invoke");
        request["params"]["edit"] = json!({"kind":"choice","value":routing});
        request["params"]["refs"] = json!([]);
        request["params"]["points"] = json!([
            {"x":40,"y":100,"attach":null}, {"x":140,"y":199,"attach":null}]);
        let mut reply = one(request.clone());
        for y in [200.1, 199.9, 210.0, 216.0] {
            let op = &reply["result"]["batch"]["operations"][0];
            assert!(op["geometry"].is_object(), "{reply}");
            request["params"]["object"]["geometry"] = op["geometry"].clone();
            request["params"]["object"]["addonData"] = json!({"connectors":op["addonData"]});
            request["params"]["edit"] = json!({"kind":"endpoint","end":"end"});
            request["params"]["points"][1]["y"] = json!(y);
            reply = one(request.clone());
            let data = &reply["result"]["batch"]["operations"][0]["addonData"]["data"];
            assert_eq!(
                data["startSide"],
                if y < 215.0 { "right" } else { "bottom" }
            );
            assert_eq!(data["endSide"], if y < 215.0 { "left" } else { "top" });
        }
    }
}
