//! `rbxtsc`: the command line without plugins. A binary with plugins is the same three lines with `.plugin(…)`
//! calls (DESIGN §11.6).

#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

fn main() -> std::process::ExitCode {
    rbxts_cli::Cli::new().run()
}
