//! Standalone UTF-8 clipboard writer.
use anyhow::Result;
#[cfg(feature = "backend")]
use zclip::backend;
use zclip::{CopyMode, parse_copy_args, remote};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "-h" | "-help" | "--help"))
    {
        println!("{}", zclip::copy_help());
        return Ok(());
    }
    let daemon = matches!(parse_copy_args(args)?, CopyMode::Daemon);
    if !daemon && remote::copy()?.is_some() {
        return Ok(());
    }
    #[cfg(feature = "backend")]
    return backend::copy(daemon);
    #[cfg(not(feature = "backend"))]
    anyhow::bail!("no Zetta clipboard channel answered; this zcopy has no local backend")
}
