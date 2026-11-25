//! Goodtype addon session v0 over stdio. The host sends the first frame, owns deadlines and the
//! process lifetime, and keeps one request in flight. Routing is fast and pure, so requests are
//! answered synchronously on this thread.
use crate::addon::{self, Refusal};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};
use std::{
    collections::HashSet,
    fmt,
    io::{BufRead, Read, Write},
};

pub const API: &str = "goodtype-api 0.1";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const PATH_EDIT: &str = "path-edit-v0";
const BATCH: &str = "page-batch-v0";
/// The frame limit before the line delimiter.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// JSON that rejects duplicate object keys at any depth.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Json;
        impl<'de> Visitor<'de> for Json {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut object = Map::new();
                while let Some((key, Strict(value))) = map.next_entry()? {
                    if object.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(Strict(Value::Object(object)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut list = Vec::new();
                while let Some(Strict(value)) = seq.next_element()? {
                    list.push(value);
                }
                Ok(Strict(Value::Array(list)))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
        }
        d.deserialize_any(Json)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    jsonrpc: String,
    #[serde(default, deserialize_with = "request_id")]
    id: Option<String>,
    method: String,
    params: Value,
}

fn request_id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

fn read_frame(input: &mut impl BufRead) -> Result<Option<Frame>> {
    let mut line = Vec::new();
    if input
        .take((MAX_LINE + 2) as u64)
        .read_until(b'\n', &mut line)?
        == 0
    {
        return Ok(None);
    }
    let payload = line
        .strip_suffix(b"\n")
        .ok_or("unterminated or oversized frame")?;
    let payload = payload.strip_suffix(b"\r").unwrap_or(payload);
    if payload.len() > MAX_LINE {
        return Err("oversized frame".into());
    }
    let Strict(value) = serde_json::from_slice(payload)?;
    // Serde would also accept a struct written as an array.
    if !value.is_object() || !value["params"].is_object() {
        return Err("frame and params must be JSON objects".into());
    }
    let frame: Frame = serde_json::from_value(value)?;
    if frame.jsonrpc != "2.0" || frame.id.as_deref().is_some_and(|id| !valid_id(id)) {
        return Err("invalid RPC envelope".into());
    }
    Ok(Some(frame))
}

/// Positive decimal string IDs, as the host issues them.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && id.bytes().any(|b| b != b'0')
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Initialize {
    api_version: String,
    capabilities: Vec<String>,
    addon: Map<String, Value>,
}

fn identity() -> Value {
    json!({"id": crate::connector::ADDON_ID, "version": VERSION})
}

/// The accepted capabilities: only `page-batch-v0`. Requests finish in milliseconds, so the
/// addon declines `progress` and `cancel`.
fn initialize(params: Value) -> Option<Vec<&'static str>> {
    let params: Initialize = serde_json::from_value(params).ok()?;
    let unique: HashSet<_> = params.capabilities.iter().map(String::as_str).collect();
    let ok = params.api_version == API
        && Value::Object(params.addon) == identity()
        && unique.len() == params.capabilities.len()
        && unique.contains(BATCH)
        && unique.contains(PATH_EDIT);
    ok.then(|| vec![BATCH, PATH_EDIT])
}

fn error(code: i32, message: &str) -> Value {
    json!({"error": {"code": code, "message": message}})
}

fn reply(output: &mut impl Write, id: &str, mut body: Value) -> Result<()> {
    body["jsonrpc"] = json!("2.0");
    body["id"] = json!(id);
    let frame = serde_json::to_vec(&body)?;
    if frame.len() > MAX_LINE {
        return Err("reply exceeds frame limit".into());
    }
    output.write_all(&frame)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn dispatch(method: &str, params: Value) -> Value {
    let outcome = match method {
        "tool/apply" => addon::tool_apply(params),
        "page/changed" => addon::page_changed(params),
        "command/invoke" => addon::command_invoke(params),
        _ => return error(-32601, "Unknown method"),
    };
    match outcome {
        Ok(result) => json!({"result": result}),
        Err(Refusal::Invalid(message)) => error(-32602, message),
        Err(Refusal::Failed(message)) => error(-32001, message),
    }
}

/// Serve one session until the host closes stdin. A framing or sequencing violation ends it
/// with an error; a refused request leaves it usable.
pub fn run(mut input: impl BufRead, mut output: impl Write) -> Result<()> {
    let mut initialized = false;
    let mut seen = HashSet::new();
    let mut id_bytes = 0;
    while let Some(frame) = read_frame(&mut input)? {
        let Some(id) = frame.id else {
            // No notification is negotiated (no cancel or progress).
            return Err("unexpected notification".into());
        };
        id_bytes += id.len();
        if seen.len() == 65_536 || id_bytes > MAX_LINE || !seen.insert(id.clone()) {
            return Err("repeated request ID or session ID budget exhausted".into());
        }
        if frame.method == "initialize" {
            if initialized {
                return Err("repeated initialization".into());
            }
            let Some(accepted) = initialize(frame.params) else {
                reply(
                    &mut output,
                    &id,
                    error(-32602, "Incompatible initialization"),
                )?;
                return Err("incompatible initialization".into());
            };
            reply(
                &mut output,
                &id,
                json!({"result": {"apiVersion": API, "capabilities": accepted, "addon": identity()}}),
            )?;
            initialized = true;
        } else if !initialized {
            return Err("request before initialization".into());
        } else {
            reply(&mut output, &id, dispatch(&frame.method, frame.params))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn handshake() -> Value {
        json!({"apiVersion": API, "capabilities": ["progress", "cancel", BATCH, PATH_EDIT],
            "addon": {"id": "connectors", "version": VERSION}})
    }

    fn request(id: &str, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    pub(crate) fn exchange(frames: &[Value]) -> (bool, Vec<Value>) {
        let input: String = frames.iter().map(|frame| format!("{frame}\n")).collect();
        let mut output = Vec::new();
        let ok = run(Cursor::new(input), &mut output).is_ok();
        let rows = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (ok, rows)
    }

    #[test]
    fn handshake_accepts_batch_and_path_edit() {
        let (ok, rows) = exchange(&[request("7", "initialize", handshake())]);
        assert!(ok);
        assert_eq!(
            rows[0],
            json!({"jsonrpc": "2.0", "id": "7", "result": {"apiVersion": API,
                "capabilities": [BATCH, PATH_EDIT], "addon": {"id": "connectors", "version": VERSION}}})
        );
    }

    #[test]
    fn mismatched_handshakes_end_the_session() {
        for (key, value) in [
            ("apiVersion", json!("goodtype-api 0.2")),
            ("addon", json!({"id": "math", "version": VERSION})),
            ("addon", json!({"id": "connectors", "version": "9.9.9"})),
            ("capabilities", json!(["cancel"])),
            ("capabilities", json!([BATCH])),
            ("capabilities", json!([BATCH, BATCH])),
        ] {
            let mut params = handshake();
            params[key] = value;
            let (ok, rows) = exchange(&[request("1", "initialize", params)]);
            assert!(!ok);
            assert_eq!(rows[0]["error"]["code"], -32602);
        }
    }

    #[test]
    fn sequencing_errors_end_the_session() {
        let (ok, rows) = exchange(&[request("1", "page/changed", json!({}))]);
        assert!(!ok && rows.is_empty());
        let (ok, rows) = exchange(&[
            request("1", "initialize", handshake()),
            request("1", "page/changed", json!({})),
        ]);
        assert!(!ok && rows.len() == 1);
        let (ok, _) = exchange(&[
            request("1", "initialize", handshake()),
            json!({"jsonrpc": "2.0", "method": "command/cancel", "params": {"id": "1"}}),
        ]);
        assert!(!ok);
    }

    #[test]
    fn refused_requests_keep_the_session_usable() {
        let (ok, rows) = exchange(&[
            request("1", "initialize", handshake()),
            request("2", "absent", json!({})),
            request("3", "tool/apply", json!({"tool": "connect"})),
            request(
                "4",
                "page/changed",
                json!({"page": {"widthPt": 595, "heightPt": 842},
                "changed": [], "deleted": [], "own": []}),
            ),
        ]);
        assert!(ok);
        assert_eq!(rows[1]["error"]["code"], -32601);
        assert_eq!(rows[2]["error"]["code"], -32602);
        assert_eq!(rows[3]["result"], json!({"batch": {"operations": []}}));
    }

    #[test]
    fn strict_framing() {
        for input in [
            b"{}".to_vec(),
            b"[\"2.0\",\"1\",\"initialize\",{}]
".to_vec(),
            vec![b' '; MAX_LINE + 4],
            b"{\"id\":\"1\",\"id\":\"2\"}\n".to_vec(),
            b"{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"method\":\"initialize\",\"params\":{\"a\":1,\"\\u0061\":2}}\n".to_vec(),
            b"{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"method\":\"initialize\",\"params\":{},\"extra\":true}\n".to_vec(),
            b"{\"jsonrpc\":\"1.0\",\"id\":\"1\",\"method\":\"initialize\",\"params\":{}}\n".to_vec(),
            b"{\"jsonrpc\":\"2.0\",\"id\":\"01a\",\"method\":\"initialize\",\"params\":{}}\n".to_vec(),
            b"{\"jsonrpc\":\"2.0\",\"id\":\"0\",\"method\":\"initialize\",\"params\":{}}\n".to_vec(),
        ] {
            assert!(read_frame(&mut Cursor::new(input)).is_err());
        }
        let mut exact = request("1", "initialize", handshake())
            .to_string()
            .into_bytes();
        exact.resize(MAX_LINE, b' ');
        exact.extend_from_slice(b"\r\n");
        assert!(read_frame(&mut Cursor::new(&exact)).unwrap().is_some());
        exact.insert(0, b' ');
        assert!(read_frame(&mut Cursor::new(&exact)).is_err());
    }

    #[test]
    fn oversized_reply_writes_no_partial_frame() {
        let mut output = Vec::new();
        let id = "1".repeat(MAX_LINE);
        assert!(reply(&mut output, &id, error(-32602, "Invalid request")).is_err());
        assert!(output.is_empty());
    }
}
