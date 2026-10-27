//! Prints, for each Luau file given, whether the pinned compiler accepts it at optimisation levels 0–2 and debug
//! levels 1–2 (`+` accepted, `-` refused), and the first refusal: the same layout as the Lune loader the limit probes
//! use, to compare the two compilers. `cargo run -p rbxts_luau_check --example matrix -- <file>...`

fn main() {
    for path in std::env::args().skip(1) {
        let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
        let mut line = name.to_owned();
        let mut first = None;
        for debug_level in [1, 2] {
            line.push_str(&format!("  d{debug_level}:"));
            for optimization_level in [0, 1, 2] {
                match rbxts_luau_check::compile(&source, optimization_level, debug_level) {
                    Ok(()) => line.push('+'),
                    Err(error) => {
                        line.push('-');
                        first.get_or_insert(format!("{}: {}", error.line, error.message));
                    }
                }
            }
        }
        if let Some(first) = first {
            line.push_str(&format!("  {}", first.chars().take(100).collect::<String>()));
        }
        println!("{line}");
    }
}
