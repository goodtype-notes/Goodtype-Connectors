use goodtype_connectors::{addon, session};
use serde_json::json;
use std::io::{self, BufReader};

/// A fixed connect gesture between two parts, run by `--check` and the package smoke test.
fn self_check() -> bool {
    let part = |id: &str, x: f64| {
        json!({"id": id, "type": "shape", "bounds": {"x": x, "y": 100, "width": 48, "height": 16},
            "ports": [{"x": x, "y": 108}, {"x": x + 48.0, "y": 108}]})
    };
    ["straight", "curved", "orthogonal"].iter().all(|routing| {
        let params = json!({"tool": "connect", "settings": {"routing": routing, "arrow": "end"},
            "page": {"widthPt": 595.28, "heightPt": 841.89},
            "points": [{"x": 148, "y": 108, "attach": {"objectId": "a", "port": 1}},
                       {"x": 300, "y": 108, "attach": {"objectId": "b", "port": 0}}],
            "objects": [part("a", 100.0), part("b", 300.0)]});
        addon::tool_apply(params).is_ok()
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--version", "--json"] => println!(
            "{}",
            json!({"name": "goodtype-connectors",
                "version": option_env!("GOODTYPE_BUILD_VERSION").unwrap_or(session::VERSION),
                "protocolVersion": 1})
        ),
        ["--check"] => {
            let ok = self_check();
            println!("{}", json!({"ok": ok}));
            if !ok {
                std::process::exit(1);
            }
        }
        ["--session"] => {
            std::panic::set_hook(Box::new(|_| eprintln!("Connectors addon internal error")));
            if session::run(BufReader::new(io::stdin().lock()), io::stdout().lock()).is_err() {
                eprintln!("Connectors addon session failed");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: goodtype-connectors --version --json | --check | --session");
            std::process::exit(2);
        }
    }
}
