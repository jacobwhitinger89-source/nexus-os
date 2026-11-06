//! `nexus-connect` — standalone build of `nexus connect` for workstations
//! (Linux/macOS) that do not need the rest of the `nexus` CLI.

fn main() -> std::process::ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    std::process::ExitCode::from(nexus_connect::main_with(&raw))
}
