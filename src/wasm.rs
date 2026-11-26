//! Sideload-only goodtype-wasm-v0 adapter; routing and ordinary geometry stay shared.
use extism_pdk::*;
use serde::Deserialize;
use serde_json::{Value, json};

macro_rules! ensure {
    ($condition:expr, $message:literal) => {
        if !$condition {
            return Err(Error::msg($message).into());
        }
    };
}

const LIMIT: usize = 32 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preview {
    gesture_id: String,
    seq: u64,
    revision: u64,
    kind: String,
    local: Value,
    ports: Value,
    settings: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Commit {
    revision: u64,
    gesture: String,
    scope: Value,
    settings: Value,
}

fn bounded(value: Value) -> FnResult<String> {
    let output = serde_json::to_string(&value)?;
    ensure!(output.len() <= LIMIT, "output limit");
    Ok(output)
}

fn execute(kind: &str, mut scope: Value, settings: Value) -> FnResult<Value> {
    if kind != "changed" {
        scope["settings"] = settings;
    }
    let result = match kind {
        "changed" => crate::addon::page_changed(scope),
        "tool" => crate::addon::tool_apply(scope),
        "path" => crate::addon::command_invoke(scope),
        _ => return Err(Error::msg("unknown gesture").into()),
    };
    result.map_err(|_| Error::msg("invalid connector input").into())
}

#[plugin_fn]
pub fn preview(input: String) -> FnResult<String> {
    ensure!(input.len() <= LIMIT, "input limit");
    let request: Preview = serde_json::from_str(&input)?;
    ensure!(request.gesture_id.len() <= 128, "gesture limit");
    let _stamp = (request.seq, request.revision, request.ports);
    let reply = execute(&request.kind, request.local.clone(), request.settings)?;
    // Only ordinary shape geometry is handed to the host's ephemeral renderer.
    let marks: Vec<Value> = reply["batch"]["operations"]
        .as_array()
        .ok_or_else(|| Error::msg("missing operations"))?
        .iter()
        .map(|op| {
            let source = request.local["own"].as_array().and_then(|objects| objects.iter().find(|object| object["id"] == op["id"]));
            let field = |key: &str, default: f64| op.get(key).or_else(|| source.and_then(|object| object.get(key))).cloned().unwrap_or(json!(default));
            json!({"x": field("x", 0.0), "y": field("y", 0.0), "rotation": field("rotation", 0.0), "scale": field("scale", 1.0), "geometry": op["geometry"]})
        })
        .collect();
    bounded(json!({"marks": marks}))
}

#[plugin_fn]
pub fn commit(input: String) -> FnResult<String> {
    ensure!(input.len() <= LIMIT, "input limit");
    let request: Commit = serde_json::from_str(&input)?;
    let _revision = request.revision;
    bounded(execute(&request.gesture, request.scope, request.settings)?)
}
