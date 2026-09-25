//! Standalone UTF-8 clipboard reader.
use anyhow::Result;
#[cfg(feature = "backend")]
use zclip::backend;
use zclip::{parse_paste_args, remote};
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
    if remote::paste()?.is_some() {
        return Ok(());
    }
    #[cfg(feature = "backend")]
    return backend::paste();
    #[cfg(not(feature = "backend"))]
    anyhow::bail!("no Zetta clipboard channel answered; this zpaste has no local backend")
}
