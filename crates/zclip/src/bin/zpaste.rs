//! Standalone UTF-8 clipboard reader.
use anyhow::Result;
use zclip::{backend, parse_paste_args};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "-h" | "-help" | "--help"))
    {
        println!("{}", zclip::paste_help());
        return Ok(());
    }
    parse_paste_args(args)?;
    backend::paste()
}
