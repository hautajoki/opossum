//! Checks that refused compiles don't leak: compiles a file Luau refuses (`fail`: 201 locals) or accepts (`pass`:
//! 199) `count` times, then waits three seconds so the process's memory can be read from outside (Task Manager,
//! `Get-Process`). Luau reports a refusal by throwing a C++ exception through its frames; built without unwind
//! semantics (MSVC without `/EH`), each refusal leaked about 37 KiB. With them, `fail` and `pass` stay flat.
//! `cargo run -p rbxts_luau_check --example leak -- fail 8000` (the validators' round-6 probe).

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("fail|pass");
    let count: usize = args.next().and_then(|a| a.parse().ok()).expect("count");
    let n = if mode == "fail" { 201 } else { 199 };
    let mut source: String = (0..n).map(|i| format!("local x{i} = f()\n")).collect();
    source.push_str("print(x0)\n");
    let refused = (0..count).filter(|_| rbxts_luau_check::compile(&source, 1, 1).is_err()).count();
    println!("{mode}: {count} compiles, {refused} refused");
    std::thread::sleep(std::time::Duration::from_secs(3));
}
