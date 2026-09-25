//! Standalone UTF-8 clipboard writer.
use anyhow::Result;
use zclip::{CopyMode, backend, parse_copy_args};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "-h" | "-help" | "--help"))
    {
        println!("{}", zclip::copy_help());
        return Ok(());
    }
    backend::copy(matches!(parse_copy_args(args)?, CopyMode::Daemon))
}
