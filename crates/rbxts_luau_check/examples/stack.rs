//! Measures the stack Luau's compiler needs: compiles the deepest shapes Luau accepts (and some it refuses for depth)
//! at every level, on a thread with the given stack. A stack too small aborts the process (Windows: exit code
//! 0xC00000FD), so run it with decreasing sizes. `cargo run -p rbxts_luau_check --example stack -- <KiB>`

fn shapes() -> Vec<(&'static str, String)> {
    let nest = |n: usize, open: &str, inner: &str, close: &str| format!("{}{inner}{}", open.repeat(n), close.repeat(n));
    let chain = |n: usize, op: &str| (0..n).map(|i| format!("x{}", i % 50)).collect::<Vec<_>>().join(op);
    let mut out = Vec::new();
    for n in [990, 1200] {
        out.push(("parens", format!("local x = {}\n", nest(n, "(", "1", ")"))));
        out.push(("calls", format!("local x = {}\n", nest(n, "f(", "1", ")"))));
        out.push(("tables", format!("local x = {}\n", nest(n, "{", "1", "}"))));
        out.push(("unary", format!("local x = {}y\n", "- ".repeat(n))));
        out.push(("not", format!("local x = {}y\n", "not ".repeat(n))));
        out.push(("index", format!("local x = y{}\n", "[1]".repeat(n))));
        out.push(("blocks", format!("{}print(1)\n{}", "do\n".repeat(n), "end\n".repeat(n))));
        out.push(("whiles", format!("{}print(1)\n{}", "while y do\n".repeat(n), "end\n".repeat(n))));
        out.push(("functions", format!("local x = {}1{}\n", "function() return ".repeat(n), " end".repeat(n))));
        out.push(("ifexpr", format!("local x = {}1\n", "if y then 1 else ".repeat(n))));
        let elseifs = (0..n).map(|i| format!("elseif y == {i} then\n\tprint({i})\n")).collect::<String>();
        out.push(("elseif", format!("if y then\n\tprint(0)\n{elseifs}end\n")));
    }
    let locals = (0..50).map(|i| format!("local x{i} = f()\n")).collect::<String>();
    for n in [990, 5000] {
        out.push(("concat", format!("{locals}local s = {}\n", chain(n, " .. "))));
        out.push(("sum", format!("{locals}local s = {}\n", chain(n, " + "))));
        out.push(("and", format!("{locals}local s = {}\n", chain(n, " and "))));
    }
    out
}

fn main() {
    let kib: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).expect("a stack size in KiB");
    let shapes = shapes();
    let handle = std::thread::Builder::new()
        .stack_size(kib * 1024)
        .spawn(move || {
            for (name, source) in &shapes {
                let mut line = format!("{name:<10}");
                for (o, d) in [(0, 1), (1, 1), (2, 1), (1, 2)] {
                    line.push(if rbxts_luau_check::compile(source, o, d).is_ok() { '+' } else { '-' });
                }
                if let Err(error) = rbxts_luau_check::compile(source, 1, 1) {
                    line.push_str(&format!("  {}", error.message.chars().take(60).collect::<String>()));
                }
                println!("{line}");
            }
        })
        .expect("a thread");
    handle.join().expect("the shapes compiled");
    println!("ok at {kib} KiB");
}
