//! The mock Open Cloud as a server, for end-to-end checks of the real CLI against it through
//! `OPOSSUM_OPEN_CLOUD_URL`: `cargo run -p cloud --example mock [-- --port p]` prints its base URL and serves until
//! killed. It is an example so it never ships in the opossum binary.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port = match &args[..] {
        [] => Ok(0),
        [flag, port] if flag == "--port" => port.parse::<u16>().map_err(|_| format!("--port {port:?} is not a port")),
        _ => Err("usage: mock [--port p]".into()),
    };
    if let Err(e) = port.and_then(cloud::mock::serve) {
        eprintln!("{e}");
        std::process::exit(2);
    }
}
