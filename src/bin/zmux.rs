//! The multiplexer executable built alongside `zetta`.
//!
//! `zetta mux` dispatches to the same [`zmux::run`], so the two entry points
//! cannot drift apart.

fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let Err(error) = zmux::run(&arguments) {
        eprintln!("zmux: {error:#}");
        std::process::exit(1);
    }
}
