//! `plumb doctor` warns when it cannot overwrite its own executable (Windows + running MCP host).
use plumbgraph_core::providers::overwrite_warning;
use std::io::{Error, ErrorKind};
use std::path::Path;

#[test]
fn locked_executable_produces_actionable_advice() {
    let exe = Path::new(r"C:\Users\me\.cargo\bin\plumb.exe");
    let w = overwrite_warning(exe, |_| Err(Error::from(ErrorKind::PermissionDenied)))
        .expect("a locked exe must warn");
    assert!(w.contains(r"C:\Users\me\.cargo\bin\plumb.exe"), "{w}");
    assert!(w.contains("plumb mcp"), "{w}");
    assert!(w.contains("cargo install"), "{w}");
    assert!(w.contains("--root"), "{w}");
}

#[test]
fn writable_executable_does_not_warn() {
    assert!(overwrite_warning(Path::new("plumb.exe"), |_| Ok(())).is_none());
}

#[test]
fn unrelated_io_errors_are_reported_with_their_cause() {
    let w = overwrite_warning(Path::new("plumb"), |_| {
        Err(Error::new(ErrorKind::Other, "disk on fire"))
    })
    .unwrap();
    assert!(w.contains("disk on fire"), "{w}");
}
