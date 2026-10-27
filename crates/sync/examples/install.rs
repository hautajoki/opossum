//! `cargo run -p sync --example install -- <path.rbxm>`: writes the Studio plugin, then reads it back.

fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: install <path.rbxm>");
        std::process::exit(2);
    };
    let path = std::path::PathBuf::from(path);
    if let Err(e) = sync::install_plugin(&path) {
        eprintln!("{}: {e}", path.display());
        std::process::exit(1);
    }
    let bytes = std::fs::read(&path).unwrap_or_default();
    match rbx_binary::from_reader(bytes.as_slice()) {
        Ok(dom) => {
            for &child in dom.root().children() {
                if let Some(inst) = dom.get_by_ref(child) {
                    println!(
                        "{} {} ({} bytes in file)",
                        inst.class,
                        inst.name,
                        bytes.len()
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("{} does not parse back: {e}", path.display());
            std::process::exit(1);
        }
    }
}
