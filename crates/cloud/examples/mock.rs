//! The mock Open Cloud as a server, for end-to-end checks of the real CLI against it through
//! `OPOSSUM_OPEN_CLOUD_URL`: `cargo run -p cloud --example mock [-- --port p] [--results file.json]` prints its base
//! URL and serves until killed, every task returning the JSON array in `file.json` (none without it). It is an example
//! so it never ships in the opossum binary.

fn main() {
    if let Err(e) = options().and_then(|(port, results)| cloud::mock::serve(port, results)) {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

fn options() -> Result<(u16, Vec<serde_json::Value>), String> {
    let (mut port, mut results) = (0, Vec::new());
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("usage: mock [--port p] [--results file.json]")?;
        match flag.as_str() {
            "--port" => port = value.parse().map_err(|_| format!("--port {value:?} is not a port"))?,
            "--results" => {
                let text = std::fs::read_to_string(&value).map_err(|e| format!("{value}: {e}"))?;
                match serde_json::from_str(&text) {
                    Ok(serde_json::Value::Array(values)) => results = values,
                    _ => return Err(format!("{value} must hold a JSON array")),
                }
            }
            _ => return Err("usage: mock [--port p] [--results file.json]".into()),
        }
    }
    Ok((port, results))
}
